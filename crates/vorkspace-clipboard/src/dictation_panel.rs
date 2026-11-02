//! Compact native notch using Vox's shared placement, shape and spring.
use gpui::{prelude::*, *};
use vorkspace_macos::notch::{self, Geometry, Placement};
use vorkspace_ui::{clock::Instant, pane_motion::PaneSlide};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Starting,
    Recording,
    Transcribing,
    Error,
}
impl State {
    pub fn recording(self) -> bool {
        self == Self::Recording
    }
    pub fn error(self) -> bool {
        self == Self::Error
    }
    fn label(self) -> &'static str {
        match self {
            Self::Starting => "Microphone starting — wait for red before speaking",
            Self::Recording => "Microphone active — recording",
            Self::Transcribing => "Microphone — transcribing",
            Self::Error => "Microphone — needs attention",
        }
    }
}
pub struct Panel {
    pub status: String,
    pub state: State,
    pub placement: Placement,
    geometry: Geometry,
    _screens: Option<notch::ScreenChanges>,
    _screen_task: Option<Task<()>>,
    motion: PaneSlide,
}
pub fn open(status: &str, state: State, cx: &mut App) -> gpui::Result<WindowHandle<Panel>> {
    let geometry = Geometry::read();
    let status = status.to_string();
    cx.open_window(notch::window_options(geometry), move |_, cx| {
        cx.new(|cx| {
            let (watch, rx) = notch::ScreenChanges::new();
            let task = cx.spawn(async move |this, cx| {
                while rx.recv().await.is_ok() {
                    let _ = this.update(cx, |_, cx| cx.notify());
                }
            });
            let placement = Placement::default();
            placement.open();
            Panel {
                status,
                state,
                placement,
                geometry,
                _screens: Some(watch),
                _screen_task: Some(task),
                motion: PaneSlide::new(
                    geometry.notch_width,
                    geometry.compact_width(),
                    notch::WING_WIDTH * 2.,
                ),
            }
        })
    })
}
impl Render for Panel {
    fn render(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let geometry = Geometry::read();
        if self.geometry != geometry {
            self.motion = self
                .motion
                .retarget(geometry.compact_width(), Instant::now());
            self.geometry = geometry;
        }
        let (animated_width, done) = self.motion.at(Instant::now());
        if !done {
            w.request_animation_frame();
        }
        let error = self.state.error();
        let width = if error {
            animated_width.max(440.).min(geometry.screen_width)
        } else {
            animated_width
        };
        let height = geometry.notch_height + if error { 108. } else { 0. };
        notch::place(w, geometry, width, height, &self.placement, cx);
        let wing = ((width - geometry.notch_width) / 2.).max(0.);
        div()
            .id("dictation-notch")
            .role(Role::Group)
            .aria_label(format!("Dictation: {}", self.status))
            .debug_selector(|| "dictation-notch".into())
            .relative()
            .w(px(width))
            .h(px(height))
            .overflow_hidden()
            .text_color(rgb(0xffffff))
            .font_family(vorkspace_ui::fonts::UI)
            .child(notch::body())
            .child(
                div()
                    .relative()
                    .h(px(geometry.notch_height))
                    .flex()
                    .items_center()
                    // Deliberately empty left wing: no Vox face or name.
                    .child(div().w(px(wing)).h_full())
                    .child(div().w(px(geometry.notch_width)).h_full())
                    .child(
                        div()
                            .id("dictation-microphone")
                            .role(Role::Image)
                            .aria_label(self.state.label())
                            .w(px(wing))
                            .h_full()
                            .overflow_hidden()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(microphone(self.state.recording()))
                            .when(!self.state.recording() && !error, |d| {
                                d.child(div().text_size(px(11.)).child("…"))
                            }),
                    ),
            )
            .when(error, |d| {
                d.child(
                    div()
                        .relative()
                        .px_4()
                        .py_2()
                        .text_size(px(11.))
                        .child(self.status.clone()),
                )
            })
    }
}
fn microphone(active: bool) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, w, _| {
            let origin = bounds.origin;
            let mut p = PathBuilder::stroke(px(1.8));
            let pt = |x, y| point(origin.x + px(x), origin.y + px(y));
            p.move_to(pt(7., 4.));
            p.curve_to(pt(13., 4.), pt(10., -1.));
            p.line_to(pt(13., 10.));
            p.curve_to(pt(7., 10.), pt(10., 15.));
            p.close();
            p.move_to(pt(4., 9.));
            p.curve_to(pt(16., 9.), pt(10., 22.));
            p.move_to(pt(10., 16.));
            p.line_to(pt(10., 20.));
            p.move_to(pt(6., 20.));
            p.line_to(pt(14., 20.));
            if let Ok(p) = p.build() {
                w.paint_path(p, if active { rgb(0xf87171) } else { rgb(0xfbbf24) });
            }
        },
    )
    .w(px(20.))
    .h(px(24.))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    #[ignore = "Requires Metal and VORKSPACE_DICTATION_CAPTURE_DIR; display-only fixture"]
    fn native_dictation_notch_captures() {
        use std::sync::Arc;
        let output =
            std::path::PathBuf::from(std::env::var_os("VORKSPACE_DICTATION_CAPTURE_DIR").unwrap());
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
        let window = cx
            .open_window(size(px(480.), px(180.)), |_, cx| {
                cx.new(|_| Panel {
                    status: "Recording".into(),
                    state: State::Recording,
                    placement: Placement::default(),
                    geometry: Geometry::default(),
                    motion: PaneSlide::new(293., 293., 0.),
                    _screens: None,
                    _screen_task: None,
                })
            })
            .unwrap();
        for (name, status, state) in [
            ("starting", "Starting microphone…", State::Starting),
            ("recording", "Recording", State::Recording),
            ("transcribing", "Transcribing…", State::Transcribing),
            (
                "error",
                "Microphone access is required. Use Set up permissions in Settings → Clipboard, then allow vorkspace Clipboard in Privacy & Security.",
                State::Error,
            ),
        ] {
            window
                .update(&mut cx, |p, _, cx| {
                    p.status = status.into();
                    p.state = state;
                    cx.notify();
                })
                .unwrap();
            cx.run_until_parked();
            vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(output.join(format!("dictation-{name}.png")))
                .unwrap();
        }
    }
}
