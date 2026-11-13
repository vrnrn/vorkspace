//! One notch and one global registration across all workspace windows.
use super::*;
use gpui::{App, Global, WindowHandle};

pub(crate) const SHORTCUT_COMMAND: &str = "App: Vox";
pub(crate) const DEFAULT_SHORTCUT: &str = "ctrl-alt-space";

#[derive(Default)]
struct Controller {
    panel: Option<WindowHandle<Vox>>,
    workspace_toggle: Option<fn(&mut App)>,
    config: String,
    error: Option<String>,
    #[cfg(not(test))]
    registration: Option<vorkspace_macos::global_hotkey::Registration>,
    #[cfg(not(test))]
    task: Option<gpui::Task<()>>,
}
impl Global for Controller {}

pub(crate) fn configure(prefs: &vorkspace_ui::preferences::Preferences, cx: &mut App) {
    if !cx.has_global::<Controller>() {
        cx.set_global(Controller::default());
    }
    let shortcut = match prefs.keybindings.get(SHORTCUT_COMMAND) {
        Some(Some(shortcut)) => shortcut.as_str(),
        Some(None) => "",
        None => DEFAULT_SHORTCUT,
    };
    let config = format!("{}:{shortcut}", prefs.vox_enabled);
    if cx.global::<Controller>().config == config {
        return;
    }
    {
        let controller = cx.global_mut::<Controller>();
        controller.config = config;
        controller.error = None;
        #[cfg(not(test))]
        {
            controller.registration = None;
            controller.task = None;
        }
    }
    #[cfg(not(test))]
    if prefs.vox_enabled && !shortcut.is_empty() {
        match vorkspace_macos::global_hotkey::Registration::new(shortcut) {
            Ok((registration, receiver)) => {
                let task = cx.spawn(async move |cx| {
                    while receiver.recv().await.is_ok() {
                        cx.update(toggle_global);
                    }
                });
                let controller = cx.global_mut::<Controller>();
                controller.registration = Some(registration);
                controller.task = Some(task);
            }
            Err(error) => cx.global_mut::<Controller>().error = Some(error),
        }
    }
}

pub(crate) fn error(cx: &App) -> Option<String> {
    cx.try_global::<Controller>().and_then(|c| c.error.clone())
}

fn toggle_global(cx: &mut App) {
    let panel = cx.try_global::<Controller>().and_then(|c| c.panel);
    if let Some(panel) = panel
        && panel
            .update(cx, |voice, window, cx| voice.toggle_call(window, cx))
            .is_ok()
    {
        return;
    }
    if let Some(toggle) = cx
        .try_global::<Controller>()
        .and_then(|c| c.workspace_toggle)
    {
        toggle(cx);
    }
}

/// The global shortcut's fallback when no call panel exists yet.
pub(crate) fn set_workspace_toggle(toggle: fn(&mut App), cx: &mut App) {
    cx.default_global::<Controller>().workspace_toggle = Some(toggle);
}

pub(crate) fn toggle(voice: gpui::Entity<Vox>, cx: &mut App) {
    if !cx.has_global::<Controller>() {
        cx.set_global(Controller::default());
    }
    if let Some(panel) = cx.global::<Controller>().panel {
        if panel
            .update(cx, |voice, window, cx| voice.toggle_call(window, cx))
            .is_ok()
        {
            return;
        }
        cx.global_mut::<Controller>().panel = None;
    }
    if !voice.read(cx).enabled || voice.read(cx).running() {
        return;
    }
    // Acquire before creating any native surface, so the active owner never flashes over.
    if voice.read(cx).voice_lease.is_none() {
        match vorkspace_macos::voice_lease::Lease::acquire() {
            Ok(lease) => voice.update(cx, |v, _| v.voice_lease = Some(lease)),
            Err(error) => {
                voice.update(cx, |v, cx| {
                    v.error = Some(error);
                    cx.notify();
                });
                return;
            }
        }
    }
    let geometry = native::Geometry::read();
    let width = geometry.notch_width;
    let height = geometry.notch_height;
    let root = voice.clone();
    let result = cx.open_window(native::window_options(geometry), move |window, cx| {
        root.update(cx, |voice, cx| {
            voice.notch.open(geometry);
            voice.attach_panel(window, cx);
            native::place(window, geometry, width, height, &voice.notch.placement, cx);
            voice.start(window, cx);
        });
        root
    });
    match result {
        Ok(panel) => {
            cx.global_mut::<Controller>().panel = Some(panel);
            voice.update(cx, |voice, _| voice.panel = Some(panel));
        }
        Err(error) => voice.update(cx, |voice, cx| {
            voice.voice_lease = None;
            voice.error = Some(format!("Could not open Vox: {error}"));
            cx.notify();
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[gpui::test]
    fn global_shortcut_stops_the_existing_panel_without_activating_the_workspace(
        cx: &mut gpui::TestAppContext,
    ) {
        vorkspace_ui::clock::use_test_clock(cx);
        let store = Arc::new(std::sync::Mutex::new(
            vorkspace_core::store::Store::test(std::path::Path::new(":memory:")).unwrap(),
        ));
        let panel = cx.update(|cx| {
            cx.open_window(
                gpui::WindowOptions {
                    focus: false,
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|_| {
                        let prefs = vorkspace_ui::preferences::Preferences {
                            vox_enabled: true,
                            ..Default::default()
                        };
                        let voice =
                            Vox::new(std::rc::Rc::new(NoHost), store, &prefs, "/project".into());
                        voice.active.store(true, Ordering::Relaxed);
                        voice
                    })
                },
            )
            .unwrap()
        });
        cx.update(|cx| {
            cx.set_global(Controller {
                panel: Some(panel),
                ..Default::default()
            });
            let active = cx.active_window();
            toggle_global(cx);
            assert_eq!(active, cx.active_window());
        });
        panel
            .update(cx, |voice, _, _| {
                assert!(!voice.active.load(Ordering::Relaxed));
                assert!(voice.notch.closing);
            })
            .unwrap();
    }
}
