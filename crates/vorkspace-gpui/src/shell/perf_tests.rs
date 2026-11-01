//! Opt-in end-to-end cost samples for `npm run benchmark:native`. The real shell
//! renders offscreen through Metal against generated history in temporary data.
use super::*;
use std::{fmt::Write, time::Instant};
use vorkspace_core::{store::Store, transcript::Row};

const SESSIONS: usize = 300;
const HISTORY_ROWS: usize = 4_000;
const FRAMES: usize = 60;
const PROJECT: &str = "/tmp/vorkspace-perf";

fn row(index: usize) -> Row {
    let text = if index.is_multiple_of(2) {
        format!("Please review change {index} and explain what it affects.")
    } else {
        format!(
            "Change {index} touches **two** modules:\n\n- `store` keeps its contract\n- the view redraws once\n\n```rust\nfn change_{index}() -> usize {{\n    {index}\n}}\n```\n\nNothing else moves. {}",
            "The reasoning continues across several wrapped lines of ordinary prose. ".repeat(6)
        )
    };
    Row {
        role: if index.is_multiple_of(2) {
            "user"
        } else {
            "assistant"
        }
        .into(),
        text,
        first: true,
        sent_at: None,
    }
}

fn median(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.
}

#[test]
#[ignore = "Requires Metal and VORKSPACE_PERF_OUTPUT"]
fn native_perf_sample() {
    let output = std::env::var_os("VORKSPACE_PERF_OUTPUT").unwrap();
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("vorkspace.db");
    let mut metrics = Vec::new();

    let mut store = Store::open(&database).unwrap();
    let rows: Vec<_> = (0..HISTORY_ROWS).map(row).collect();
    let mut large = store.create("codex", PROJECT).unwrap();
    large.has_messages = true;
    let start = Instant::now();
    store.write_rows(large.id, 0, &rows).unwrap();
    metrics.push(("store_write_history", ms(start)));
    let mut summaries = vec![large.clone()];
    for _ in 1..SESSIONS {
        let mut session = store.create("codex", PROJECT).unwrap();
        store.write_rows(session.id, 0, &[row(0), row(1)]).unwrap();
        session.has_messages = true;
        summaries.push(session);
    }
    drop(store);

    let start = Instant::now();
    let store = Store::open(&database).unwrap();
    metrics.push(("store_open", ms(start)));
    let start = Instant::now();
    store.sessions().unwrap();
    metrics.push(("store_list_sessions", ms(start)));
    let store = Arc::new(Mutex::new(store));

    let mut cx = gpui::HeadlessAppContext::with_platform(
        Arc::new(gpui_apple::AppleTextSystem::new()),
        Arc::new(crate::assets::Assets),
        || {
            Ok(Some(Box::new(
                gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
            )))
        },
    );
    vorkspace_ui::clock::use_test_clock(&cx);
    let mut state = Workspace::default();
    state.add_project(PROJECT);
    state.project = PROJECT.into();
    let saved = serde_json::to_string(&state).unwrap();

    // Only main-thread work counts here. Git and update probes run on the thread
    // pool in the app, but inline while this deterministic executor parks.
    let start = Instant::now();
    let window = cx
        .open_window(gpui::size(px(1400.), px(900.)), |_, cx| {
            cx.new(|cx| Shell::new(store, summaries, PROJECT.into(), Some(saved), cx))
        })
        .unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();
    metrics.push(("shell_first_frame", ms(start)));
    cx.run_until_parked();

    // History loads on a worker thread that parking does not await.
    let start = Instant::now();
    window
        .update(&mut cx, |shell, window, cx| {
            shell.open(large.id, window, cx)
        })
        .unwrap();
    let transcript = loop {
        cx.run_until_parked();
        let loaded = window
            .update(&mut cx, |shell, _, cx| {
                shell.sessions.get(&large.id).and_then(|session| {
                    let transcript = session.read(cx).transcript.clone();
                    (!transcript.read(cx).source_rows().is_empty()).then_some(transcript)
                })
            })
            .unwrap();
        if let Some(transcript) = loaded {
            break transcript;
        }
        assert!(start.elapsed().as_secs() < 120, "history never loaded");
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();
    metrics.push(("session_open_large", ms(start)));
    assert!(transcript.read_with(&cx, |view, _| view.source_rows().len()) >= HISTORY_ROWS / 2);
    assert!(
        transcript.read_with(&cx, |view, _| view.rendered) > 0,
        "the benchmark must measure a drawn transcript"
    );

    let frame = |cx: &mut gpui::HeadlessAppContext,
                 act: &dyn Fn(&mut Shell, &mut Context<Shell>)| {
        let start = Instant::now();
        window
            .update(cx, |shell, window, cx| {
                act(shell, cx);
                window.simulate_next_frame(cx);
                window.refresh();
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .unwrap();
        ms(start)
    };
    let idle: Vec<_> = (0..FRAMES).map(|_| frame(&mut cx, &|_, _| {})).collect();
    metrics.push(("frame_redraw", median(idle)));
    let typed = (0..FRAMES)
        .map(|index| {
            let id = large.id;
            let text = "typing in the composer ".repeat(1 + index / 4);
            frame(&mut cx, &move |shell, cx| {
                let input = shell.sessions[&id].read(cx).input.clone();
                input.update(cx, |input, cx| input.set_text(text.clone(), cx));
            })
        })
        .collect();
    metrics.push(("frame_composer_input", median(typed)));
    let appended = (0..FRAMES)
        .map(|index| {
            let transcript = transcript.clone();
            let source = transcript.read_with(&cx, |view, _| view.source_rows().len());
            frame(&mut cx, &move |_, cx| {
                transcript.update(cx, |view, cx| {
                    view.patch(source, vec![row(index * 2 + 1)], cx)
                });
            })
        })
        .collect();
    metrics.push(("frame_stream_append", median(appended)));

    if let Some(path) = std::env::var_os("VORKSPACE_PERF_CAPTURE") {
        cx.capture_screenshot(window.into())
            .unwrap()
            .save(path)
            .unwrap();
    }
    let mut json = String::from("{");
    for (index, (name, value)) in metrics.iter().enumerate() {
        let comma = if index == 0 { "" } else { "," };
        write!(json, "{comma}\"{name}\":{value:.3}").unwrap();
    }
    json.push('}');
    std::fs::write(output, json).unwrap();
}
