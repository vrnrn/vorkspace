//! Explicit public-WAV fixture through the production warm worker. Never opens a mic.
use crate::dictation_worker::{self as worker, Event, Pipe, Turn};
use std::{
    path::Path,
    sync::mpsc,
    time::{Duration, Instant},
};
use transcribe_cpp::CancelToken;
pub fn run(model: &Path, wav: &Path) {
    let wav = hound::WavReader::open(wav).unwrap();
    assert_eq!(wav.spec().sample_rate, 16000);
    assert_eq!(wav.spec().channels, 1);
    let pcm: Vec<_> = wav
        .into_samples::<i16>()
        .map(|v| v.unwrap() as f32 / 32768.)
        .collect();
    assert!(pcm.len() <= worker::MAX_SAMPLES);
    let (tx, rx) = mpsc::sync_channel(1);
    let (out, events) = async_channel::bounded(32);
    let cancel = CancelToken::new();
    let worker_cancel = cancel.clone();
    let path = model.to_owned();
    let cold = Instant::now();
    let handle = std::thread::spawn(move || {
        worker::run(crate::ModelId::default(), &path, &worker_cancel, rx, out)
    });
    assert!(matches!(events.recv_blocking().unwrap(), Event::Ready));
    println!(
        "prepare_ms={} loaded_models={}",
        cold.elapsed().as_millis(),
        worker::model_loads()
    );
    for epoch in 1..=2 {
        let (pipe, input) = Pipe::new();
        tx.send(Turn {
            epoch,
            input,
            cancel: CancelToken::new(),
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
        pipe.finish().unwrap();
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
    handle.join().unwrap().unwrap();
    assert_eq!(worker::model_loads(), 1);
    println!(
        "turns=2 loaded_models={} joined=true",
        worker::model_loads()
    );
}
