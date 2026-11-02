//! No-microphone multiprocess fixture through the production Service owner.
use super::*;
use std::io::{BufRead, Write};

pub fn run(lock: PathBuf, model: PathBuf, selected: ModelId) {
    assert!(lock.is_absolute() && model.is_absolute());
    let (commands, incoming) = async_channel::bounded(2);
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if commands.send_blocking(line).is_err() {
                break;
            }
        }
    });
    gpui::Application::with_platform(std::rc::Rc::new(gpui_macos::MacPlatform::new(false)))
        .with_windowing(gpui::WindowingRequest::Headless)
        .run(move |cx| {
            cx.set_global(crate::Helper::default());
            let service = cx.new(|_| {
                let mut s = Service::new(Ok(model));
                s.fixture_engine_lock = Some(lock);
                s.initializing = false;
                s.attempt.enabled = true;
                s.selected = selected;
                s
            });
            service.update(cx, |s, cx| s.prepare_worker(cx));
            cx.spawn(async move |cx| {
                let mut last = "";
                let mut stopping = false;
                loop {
                    let (ready, running, pending, status) = service.read_with(cx, |s, _| {
                        (s.ready, s.worker.running, s.retry_pending, s.status.clone())
                    });
                    let state = if stopping && !running {
                        "JOINED"
                    } else if ready {
                        "READY"
                    } else if pending {
                        "WAITING"
                    } else if !running {
                        "FAILED"
                    } else {
                        "PREPARING"
                    };
                    if state != last {
                        println!("{state} loads={} {status}", worker::model_loads());
                        std::io::stdout().flush().unwrap();
                        last = state;
                    }
                    if state == "JOINED" {
                        cx.update(|cx| cx.quit());
                        break;
                    }
                    if let Ok(command) = incoming.try_recv() {
                        assert_eq!(command, "stop");
                        stopping = true;
                        service.update(cx, |s, _| {
                            s.attempt.enabled = false;
                            s.retire_worker();
                        });
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(20))
                        .await;
                }
            })
            .detach();
        });
}
