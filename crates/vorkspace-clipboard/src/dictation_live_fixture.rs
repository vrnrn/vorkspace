//! Explicit public-WAV fixture through the production warm worker. Never opens a mic.
use crate::dictation_engine_lease::{AcquireError, EngineLease};
use crate::dictation_worker::{self as worker, Event, Pipe, Turn};
use std::{
    path::Path,
    sync::mpsc,
    time::{Duration, Instant},
};
use transcribe_cpp::CancelToken;

fn fixture_lease() -> EngineLease {
    match std::env::var_os("VORKSPACE_DICTATION_FIXTURE_LOCK") {
        Some(path) => {
            let path = std::path::PathBuf::from(path);
            assert!(path.is_absolute());
            EngineLease::acquire_at(&path).expect("isolated fixture model lease")
        }
        None => EngineLease::acquire().expect("model lease"),
    }
}

/// Compare real streaming with a separate offline pass, serially on one warm
/// Unified model. Inputs are explicitly supplied public/generated WAV files.
pub fn compare_unified(lock: &Path, model: &Path, wavs: &[std::path::PathBuf]) {
    assert!(lock.is_absolute() && model.is_absolute() && !wavs.is_empty());
    let _lease = EngineLease::acquire_at(lock).expect("isolated fixture model lease");
    let cancel = CancelToken::new();
    let started = Instant::now();
    let warm = worker::WarmModel::load(crate::ModelId::UnifiedEnglish, model, &cancel)
        .expect("prepare Unified fixture model");
    println!(
        "prepare_ms={} model_loads={}",
        started.elapsed().as_millis(),
        worker::model_loads()
    );
    for (index, wav) in wavs.iter().enumerate() {
        assert!(wav.is_absolute());
        let reader = hound::WavReader::open(wav).expect("fixture WAV");
        assert_eq!(
            (
                reader.spec().sample_rate,
                reader.spec().channels,
                reader.spec().bits_per_sample
            ),
            (16000, 1, 16)
        );
        assert_eq!(reader.spec().sample_format, hound::SampleFormat::Int);
        assert!(reader.duration() > 0 && reader.duration() as usize <= worker::SAMPLE_RATE * 20);
        let pcm: Vec<_> = reader
            .into_samples::<i16>()
            .map(|s| s.unwrap() as f32 / 32768.)
            .collect();
        assert!(!pcm.is_empty() && pcm.len() <= worker::SAMPLE_RATE * 20);
        let (tx, rx) = mpsc::channel();
        for chunk in pcm.chunks(1600) {
            tx.send(worker::Input::Audio(chunk.to_vec())).unwrap();
        }
        tx.send(worker::Input::Finish(worker::FinishReason::Automatic))
            .unwrap();
        let turn = Turn {
            epoch: index as u64 + 1,
            input: rx,
            cancel: CancelToken::new(),
            words: Vec::new(),
        };
        let mut events = Vec::new();
        let started = Instant::now();
        warm.turn(&turn, |event| {
            if let Event::Committed(_, text) = event {
                events.push(text);
            }
            Ok(())
        })
        .expect("stream fixture");
        let stream_ms = started.elapsed().as_millis();
        let started = Instant::now();
        let (offline, pnc_runtime_control) = warm
            .offline_unified_fixture(&pcm, &cancel)
            .expect("offline fixture");
        let offline_ms = started.elapsed().as_millis();
        let streamed = events.last().cloned().unwrap_or_default();
        println!(
            "{}",
            serde_json::json!({
                "wav": wav.file_name().unwrap().to_string_lossy(),
                "audio_ms": pcm.len() * 1000 / worker::SAMPLE_RATE,
                "streamed": streamed,
                "offline": offline,
                "events": events,
                "stream_compute_ms": stream_ms,
                "offline_compute_ms": offline_ms,
                "pnc_runtime_control": pnc_runtime_control,
            })
        );
    }
    assert_eq!(worker::model_loads(), 1);
    println!("model_loads=1 worker=synchronous microphone=false destination=false");
}

/// Explicit public-audio inspection of TDT's offline windows and word timing.
/// This does not open the microphone or access a destination field.
pub fn inspect_tdt_windows(model: &Path, wav: &Path) {
    use transcribe_cpp::{Backend, Model, ModelOptions, RunOptions, TimestampKind};

    let _lease = fixture_lease();
    let reader = hound::WavReader::open(wav).expect("public WAV");
    assert_eq!(reader.spec().sample_rate, 16000);
    assert_eq!(reader.spec().channels, 1);
    let pcm: Vec<_> = reader
        .into_samples::<i16>()
        .map(|v| v.unwrap() as f32 / 32768.)
        .collect();
    assert!(pcm.len() <= worker::MAX_SAMPLES);
    let cancel = CancelToken::new();
    crate::dictation_model::verify(crate::ModelId::ParakeetTdtV3, model, &cancel)
        .expect("verified TDT model");
    transcribe_cpp::disable_logging();
    let warm = Instant::now();
    let model = Model::load_with(
        model,
        &ModelOptions {
            backend: Backend::Metal,
            ..Default::default()
        },
    )
    .expect("load TDT model");
    let mut session = model.session().expect("TDT session");
    session.set_cancel_token(&cancel);
    println!("load_ms={}", warm.elapsed().as_millis());
    let options = RunOptions {
        timestamps: TimestampKind::Word,
        ..Default::default()
    };
    for (start_s, end_s) in [
        (0, 2),
        (0, 3),
        (0, 4),
        (0, 6),
        (0, 8),
        (0, 10),
        (3, 10),
        (10, 22),
        (15, 27),
        (17, 30),
        (20, 32),
    ] {
        let end = (end_s * worker::SAMPLE_RATE).min(pcm.len());
        let start = start_s * worker::SAMPLE_RATE;
        if end <= start {
            continue;
        }
        let begin = Instant::now();
        let result = session
            .run(&pcm[start..end], &options)
            .expect("window inference");
        let words = result
            .words
            .iter()
            .map(|w| (&w.text, w.t0_ms, w.t1_ms))
            .collect::<Vec<_>>();
        println!(
            "window={start_s}..{end_s} duration_ms={} text={:?} words={words:?}",
            begin.elapsed().as_millis(),
            result.text
        );
    }
}

pub fn run(
    model: &Path,
    wav: &Path,
    selected: crate::ModelId,
    repeats: usize,
    turns: u64,
    gap_seconds: usize,
    words: &str,
) {
    let lease = fixture_lease();
    if selected == crate::ModelId::ParakeetTdtV3 {
        worker::enable_tdt_fixture_metrics();
    }
    if selected == crate::ModelId::UnifiedEnglish
        && std::env::var_os("VORKSPACE_DICTATION_FIXTURE_TRACE").is_some()
    {
        worker::enable_unified_fixture_trace();
    }
    assert!((1..=4).contains(&repeats) && (1..=8).contains(&turns) && gap_seconds <= 30);
    let read = |path: &Path| {
        assert!(path.is_absolute());
        let wav = hound::WavReader::open(path).unwrap();
        assert_eq!(wav.spec().sample_rate, 16000);
        assert_eq!(wav.spec().channels, 1);
        let original: Vec<_> = wav
            .into_samples::<i16>()
            .map(|v| v.unwrap() as f32 / 32768.)
            .collect();
        let mut pcm = Vec::new();
        for index in 0..repeats {
            if index != 0 {
                pcm.resize(pcm.len() + worker::SAMPLE_RATE * gap_seconds, 0.0);
            }
            pcm.extend_from_slice(&original);
        }
        assert!(pcm.len() <= worker::MAX_SAMPLES);
        pcm
    };
    let pcm = read(wav);
    let alternate = std::env::var_os("VORKSPACE_DICTATION_FIXTURE_ALTERNATE_WAV")
        .map(|path| read(Path::new(&path)));
    let fixtures: Vec<_> = std::env::var_os("VORKSPACE_DICTATION_FIXTURE_WAVS")
        .map(|paths| std::env::split_paths(&paths).map(|p| read(&p)).collect())
        .unwrap_or_default();
    let tails: Vec<usize> = std::env::var("VORKSPACE_DICTATION_FIXTURE_TAIL_MS")
        .ok()
        .map(|s| s.split(',').map(|v| v.parse().unwrap()).collect())
        .unwrap_or_default();
    assert!(tails.iter().all(|tail| *tail <= 3000));
    let (tx, rx) = mpsc::sync_channel(1);
    let (out, events) = async_channel::bounded(32);
    let cancel = CancelToken::new();
    let rules = vorkspace_core::dictation_words::parse(words).expect("fixture custom words");
    let worker_cancel = cancel.clone();
    let path = model.to_owned();
    let cold = Instant::now();
    let handle = std::thread::spawn(move || worker::run(selected, &path, &worker_cancel, rx, out));
    assert!(matches!(events.recv_blocking().unwrap(), Event::Ready));
    println!(
        "prepare_ms={} loaded_models={}",
        cold.elapsed().as_millis(),
        worker::model_loads()
    );
    for epoch in 1..=turns {
        let pcm = fixtures.get((epoch - 1) as usize).unwrap_or_else(|| {
            if epoch == 2 {
                alternate.as_ref().unwrap_or(&pcm)
            } else {
                &pcm
            }
        });
        let (pipe, input) = Pipe::new();
        tx.send(Turn {
            epoch,
            input,
            cancel: CancelToken::new(),
            words: rules.clone(),
        })
        .unwrap();
        assert!(matches!(events.recv_blocking().unwrap(), Event::Armed(e) if e == epoch));
        let start = Instant::now();
        let mut first = None;
        let mut delivered = worker::CommittedPrefix::default();
        let mut full = String::new();
        let mut consume = |event| {
            if let Event::Committed(e, text) = event {
                assert_eq!(e, epoch);
                if first.is_none() {
                    first = Some(start.elapsed());
                }
                let delta = delivered.advance(&text).unwrap();
                full.push_str(&delta);
                println!(
                    "turn={epoch} committed_ms={} text={text:?}",
                    start.elapsed().as_millis()
                );
            }
        };
        for (i, chunk) in pcm.chunks(1600).enumerate() {
            let deadline = start + Duration::from_millis((i as u64 + 1) * 100);
            if let Some(wait) = deadline.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
            pipe.audio(chunk.to_vec()).unwrap();
            while let Ok(event) = events.try_recv() {
                consume(event);
            }
        }
        let stop = Instant::now();
        let tail_ms = tails.get((epoch - 1) as usize).copied().unwrap_or(0);
        println!("turn={epoch} stop tail_ms={tail_ms}");
        for chunk in vec![0.; worker::SAMPLE_RATE * tail_ms / 1000].chunks(worker::SAMPLE_RATE) {
            pipe.audio(chunk.to_vec()).unwrap();
        }
        pipe.finish(worker::FinishReason::UserStop).unwrap();
        loop {
            match events.recv_blocking().unwrap() {
                Event::Done(e, result) => {
                    assert_eq!(e, epoch);
                    result.unwrap();
                    break;
                }
                event => consume(event),
            }
        }
        let first = first.expect("must emit stable text");
        assert!(
            first < stop.duration_since(start),
            "no live output before stop"
        );
        println!(
            "turn={epoch} first_stable_ms={} recording_ms={} stop_to_final_ms={} final={full:?}",
            first.as_millis(),
            stop.duration_since(start).as_millis(),
            stop.elapsed().as_millis()
        );
    }
    cancel.cancel();
    drop(tx);
    lease.join(handle).unwrap().unwrap();
    assert_eq!(worker::model_loads(), 1);
    println!(
        "turns={turns} loaded_models={} joined=true",
        worker::model_loads()
    );
}

/// Run an actual model worker in a separate process with an explicit isolated
/// lock. The caller controls cancellation and a barrier just before join.
pub fn engine_owner(lock: &Path, model: &Path, selected: crate::ModelId) {
    use std::io::{BufRead, Write};
    assert!(lock.is_absolute() && model.is_absolute());
    let lease = match EngineLease::acquire_at(lock) {
        Ok(lease) => lease,
        Err(AcquireError::Busy) => {
            println!("BUSY");
            return;
        }
        Err(error) => panic!("model lease: {error:?}"),
    };
    let cancel = CancelToken::new();
    let worker_cancel = cancel.clone();
    let (tx, rx) = mpsc::sync_channel(1);
    let (events, incoming) = async_channel::bounded(32);
    let (stopped_tx, stopped_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let path = model.to_owned();
    let handle = std::thread::spawn(move || {
        let result = worker::run(selected, &path, &worker_cancel, rx, events);
        stopped_tx
            .send(result.as_ref().map(|_| ()).map_err(Clone::clone))
            .unwrap();
        release_rx.recv().unwrap();
        result
    });
    let mut input = std::io::stdin().lock();
    let mut line = String::new();
    match incoming.recv_blocking() {
        Ok(Event::Ready) => {
            println!("READY");
            std::io::stdout().flush().unwrap();
            input.read_line(&mut line).unwrap();
            let mut active = None;
            if line.trim() == "active" {
                let (pipe, turn_input) = Pipe::new();
                let turn_cancel = CancelToken::new();
                tx.send(Turn {
                    epoch: 1,
                    input: turn_input,
                    cancel: turn_cancel.clone(),
                    words: Vec::new(),
                })
                .unwrap();
                assert!(matches!(incoming.recv_blocking(), Ok(Event::Armed(1))));
                pipe.audio(vec![0.; worker::SAMPLE_RATE / 10]).unwrap();
                active = Some((pipe, turn_cancel));
                println!("ACTIVE");
                std::io::stdout().flush().unwrap();
                line.clear();
                input.read_line(&mut line).unwrap();
            }
            assert_eq!(line.trim(), "cancel");
            if let Some((pipe, turn_cancel)) = active {
                turn_cancel.cancel();
                drop(pipe);
            }
            cancel.cancel();
            drop(tx);
        }
        _ => {
            drop(tx);
        }
    }
    let result = stopped_rx.recv().unwrap();
    println!(
        "STOPPED_BEFORE_JOIN {}",
        if result.is_ok() { "ok" } else { "error" }
    );
    std::io::stdout().flush().unwrap();
    line.clear();
    input.read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "join");
    release_tx.send(()).unwrap();
    let joined = lease.join(handle).unwrap();
    assert_eq!(joined.is_ok(), result.is_ok());
    println!("JOINED");
}
