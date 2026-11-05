//! The recorder owns this surface, so hiding or closing the app cannot hide capture.
use gpui::{prelude::*, *};
use std::time::Duration;
use vorkspace_macos::notch::{self, Geometry, Placement};
use vorkspace_meetings::recording::{Phase, State};

#[derive(Default)]
struct RecordingPanel(Option<WindowHandle<Panel>>);
impl Global for RecordingPanel {}

struct Panel {
    state: State,
    stop: fn(&str, &str, &mut App),
    placement: Placement,
    _screens: Option<notch::ScreenChanges>,
    _screen_task: Option<Task<()>>,
    _clock: Option<Task<()>>,
    #[cfg(test)]
    geometry: Option<Geometry>,
}

/// Called only for an admitted recorder, after it acquires the shared voice lease.
pub(crate) fn update(state: &State, stop: fn(&str, &str, &mut App), cx: &mut App) {
    if !state.phase.active() {
        close(cx);
        return;
    }
    if let Some(panel) = cx.default_global::<RecordingPanel>().0
        && panel
            .update(cx, |panel, _, cx| {
                panel.state = state.clone();
                cx.notify();
            })
            .is_ok()
    {
        return;
    }
    let state = state.clone();
    let panel = cx.open_window(notch::window_options(Geometry::read()), |_, cx| {
        cx.new(|cx| {
            let (screens, receiver) = notch::ScreenChanges::new();
            let screen_task = cx.spawn(async move |this, cx| {
                while receiver.recv().await.is_ok() {
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            });
            let clock = cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            });
            let placement = Placement::default();
            placement.open();
            Panel {
                state,
                stop,
                placement,
                _screens: Some(screens),
                _screen_task: Some(screen_task),
                _clock: Some(clock),
                #[cfg(test)]
                geometry: None,
            }
        })
    });
    match panel {
        Ok(panel) => cx.default_global::<RecordingPanel>().0 = Some(panel),
        Err(error) => eprintln!("Could not open the meeting recording indicator: {error}"),
    }
}

pub(crate) fn close(cx: &mut App) {
    if let Some(panel) = cx.default_global::<RecordingPanel>().0.take() {
        let _ = panel.update(cx, |panel, window, _| {
            // Invalidate queued AppKit placement before removing the window.
            panel.placement.close();
            window.remove_window();
        });
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        self.placement.close();
    }
}

fn duration(milliseconds: u64) -> String {
    let seconds = milliseconds / 1000;
    if seconds < 3600 {
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    } else {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    }
}

impl Render for Panel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let stop = self.stop;
        #[cfg(test)]
        let geometry = self.geometry.unwrap_or_else(Geometry::read);
        #[cfg(not(test))]
        let geometry = Geometry::read();
        let attached = geometry.notch_edges.is_some();
        let width = if attached {
            (geometry.notch_width + 184.).min(geometry.screen_width)
        } else {
            208.
        };
        let height = geometry.notch_height;
        let elapsed = duration(self.state.duration_ms());
        let stopping = self.state.phase == Phase::Finishing;
        let id = self.state.id.clone();
        let run = self.state.run.clone();
        notch::place(window, geometry, width, height, &self.placement, cx);
        div()
            .id("meeting-notch")
            .debug_selector(|| "meeting-notch".into())
            .role(Role::Group)
            .aria_label(format!("Meeting recording, {elapsed}"))
            .relative()
            .w(px(width))
            .h(px(height))
            .font_family(vorkspace_ui::fonts::UI)
            .text_size(px(12.))
            .line_height(px(18.))
            .text_color(white())
            .child(notch::body())
            .child(
                div()
                    .relative()
                    .size_full()
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(div().size(px(6.)).rounded_full().bg(rgb(0xff5f62)))
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .font_features(FontFeatures(std::sync::Arc::new(vec![(
                                        "tnum".into(),
                                        1,
                                    )])))
                                    .child(elapsed),
                            ),
                    )
                    // The camera cutout stays completely clear of controls.
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(
                                action("meeting-notch-open", "Open meeting", true)
                                    .child(open_icon())
                                    .on_click(|_, _, cx| {
                                        #[cfg(not(test))]
                                        crate::open_meeting(cx);
                                        #[cfg(test)]
                                        let _ = cx;
                                    }),
                            )
                            .child(
                                action("meeting-notch-stop", "Stop recording", !stopping)
                                    .child(div().size(px(9.)).rounded(px(1.5)).bg(rgb(0xff5f62)))
                                    .when(!stopping, |button| {
                                        button.on_click(move |_, _, cx| {
                                            if let Some(run) = &run {
                                                stop(&id, run, cx);
                                            }
                                        })
                                    }),
                            ),
                    ),
            )
    }
}

fn action(id: &'static str, label: &'static str, enabled: bool) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .aria_disabled(!enabled)
        .size(px(24.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .when(enabled, |button| {
            button.cursor_pointer().hover(|s| s.bg(rgba(0xffffff18)))
        })
        .when(!enabled, |button| button.opacity(0.4))
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
}

fn open_icon() -> impl IntoElement {
    canvas(
        |_, _, _| (),
        |bounds, _, window, _| {
            let p = |x, y| bounds.origin + point(px(x), px(y));
            let mut path = PathBuilder::stroke(px(1.3));
            path.move_to(p(7., 2.));
            path.line_to(p(12., 2.));
            path.line_to(p(12., 7.));
            path.move_to(p(12., 2.));
            path.line_to(p(5.5, 8.5));
            path.move_to(p(5., 3.));
            path.line_to(p(2., 3.));
            path.line_to(p(2., 12.));
            path.line_to(p(11., 12.));
            path.line_to(p(11., 9.));
            if let Ok(path) = path.build() {
                window.paint_path(path, white());
            }
        },
    )
    .size(px(14.))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(Entity<Panel>);
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .bg(rgb(0x30343b))
                .flex()
                .justify_center()
                .child(self.0.clone())
        }
    }

    #[::core::prelude::v1::test]
    #[ignore = "Requires Metal and VORKSPACE_MEETING_NOTCH_CAPTURE_DIR; display fixture only"]
    fn meeting_notch_captures() {
        use std::sync::Arc;
        let output = std::path::PathBuf::from(
            std::env::var_os("VORKSPACE_MEETING_NOTCH_CAPTURE_DIR").unwrap(),
        );
        assert!(output.is_absolute() && output.is_dir());
        let mut cx = gpui::HeadlessAppContext::with_platform(
            Arc::new(gpui_apple::AppleTextSystem::new()),
            Arc::new(()),
            || {
                Ok(Some(Box::new(
                    gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
                )))
            },
        );
        vorkspace_ui::clock::use_test_clock(&cx);
        let panel = cx.new(|_| Panel {
            state: State {
                started_at: 1,
                run_started_at: Some(1),
                ended_at: Some(764001),
                phase: Phase::Recording,
                ..State::dormant("fixture".into())
            },
            stop: |_, _, _| {},
            placement: Placement::default(),
            _screens: None,
            _screen_task: None,
            _clock: None,
            geometry: Some(Geometry::default()),
        });
        let window = cx
            .open_window(size(px(480.), px(84.)), |_, cx| {
                cx.new(|_| Fixture(panel.clone()))
            })
            .unwrap();
        for attached in [true, false] {
            for (name, phase) in [
                ("recording", Phase::Recording),
                ("finishing", Phase::Finishing),
            ] {
                panel.update(&mut cx, |panel, cx| {
                    panel.state.phase = phase;
                    panel.geometry = Some(Geometry {
                        notch_edges: attached.then_some((620., 820.)),
                        ..Default::default()
                    });
                    cx.notify();
                });
                cx.run_until_parked();
                vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(output.join(format!(
                        "meeting-notch-{}-{name}.png",
                        if attached { "notched" } else { "fallback" }
                    )))
                    .unwrap();
            }
        }
    }
}
