//! Opt-in native window probe. No account, microphone, provider, or saved data.
use super::*;
use gpui::{AppContext, WindowOptions};
use std::time::Duration;

pub(crate) fn run() {
    eprintln!("Vox display preview: no microphone or provider; exits after one state cycle.");
    gpui::Application::with_platform(std::rc::Rc::new(gpui_macos::MacPlatform::new(false)))
        .with_assets(crate::assets::Assets)
        .run(|cx| {
            crate::input::bind(cx);
            let geometry = native::Geometry::read();
            let store = Arc::new(std::sync::Mutex::new(
                vorkspace_core::store::Store::open_vorkspace(std::path::Path::new(":memory:")).unwrap(),
            ));
            let panel = cx.open_window(
                WindowOptions {
                    titlebar: None,
                    kind: gpui::WindowKind::PopUp,
                    focus: false,
                    show: false,
                    inactive_frame_interval: None,
                    is_movable: false,
                    is_resizable: false,
                    window_background: gpui::WindowBackgroundAppearance::Transparent,
                    ..Default::default()
                },
                move |window, cx| {
                    let voice = cx.new(|_| {
                        let prefs = crate::settings::Preferences { vox_enabled: true, ..Default::default() };
                        let mut voice = Vox::new(WeakEntity::new_invalid(), store, &prefs, "/preview".into());
                        voice.notch.open(geometry);
                        voice.face = "o-o".into();
                        voice.state = "connecting".into();
                        voice.active.store(true, Ordering::Relaxed);
                        voice
                    });
                    voice.update(cx, |voice, cx| {
                        voice.attach_panel(window, cx);
                        native::place(window, geometry, geometry.notch_width, geometry.notch_height, &voice.notch.placement, cx);
                    });
                    voice
                },
            ).expect("open native Vox preview");
            panel.update(cx, |voice, _, _| voice.panel = Some(panel)).unwrap();
            cx.on_window_closed(|cx, _| { if cx.windows().is_empty() { cx.quit(); } }).detach();
            cx.spawn(async move |cx| {
                for (phase, activity, text) in [
                    ("Listening", "listening", ""),
                    ("Thinking", "thinking", ""),
                    ("Speaking", "replying", ""),
                    ("Silent", "listening", "Vox lives at your notch now. Keep working in any app — I’ll show the answer here without speaking."),
                ] {
                    cx.background_executor().timer(Duration::from_secs(3)).await;
                    let _ = panel.update(cx, |voice, _, cx| {
                        voice.state = "live".into();
                        voice.notch.activity = activity.into();
                        voice.face = if activity == "thinking" { "p-p" } else { "o-o" }.into();
                        voice.silent = !text.is_empty();
                        voice.reply = text.into();
                        voice.reply_id = phase.into();
                        cx.notify();
                    });
                    cx.background_executor().timer(Duration::from_millis(500)).await;
                    if let Ok(snapshot) = panel.update(cx, |_, window, _| native::snapshot(window)) {
                        eprintln!("{phase}: {snapshot}");
                        let top = snapshot["top"].as_f64().expect("native panel frame");
                        assert!((top - geometry.top).abs() < 1., "{phase}: detached from the screen top");
                    }
                }
                cx.background_executor().timer(Duration::from_secs(9)).await;
                let _ = panel.update(cx, |voice, _, cx| voice.stop(cx));
                cx.background_executor().timer(Duration::from_secs(2)).await;
                cx.update(|cx| cx.quit());
            }).detach();
        });
}
