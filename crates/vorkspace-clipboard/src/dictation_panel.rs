//! Compact right-hand dictation indicator, independent of Vox's notch content.
use gpui::{prelude::*, *};
use vorkspace_macos::notch::{self, Geometry, Placement};
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
    #[cfg(test)]
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
    _screens: Option<notch::ScreenChanges>,
    _screen_task: Option<Task<()>>,
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
                _screens: Some(watch),
                _screen_task: Some(task),
            }
        })
    })
}
impl Render for Panel {
    fn render(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let geometry = Geometry::read();
        let width =
            notch::WING_WIDTH.min((geometry.screen_width - geometry.notch_width).max(0.) / 2.);
        let height = geometry.notch_height;
        notch::place_right(w, geometry, width, height, &self.placement, cx);
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
            .child(right_body())
            .child(
                div()
                    .relative()
                    .h(px(geometry.notch_height))
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .id("dictation-microphone")
                            .role(Role::Image)
                            .aria_label(self.state.label())
                            .w(px(width))
                            .h_full()
                            .overflow_hidden()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(microphone(self.state.recording())),
                    ),
            )
    }
}
fn right_body() -> impl IntoElement {
    canvas(
        |_, _, _| (),
        |bounds, _, w, _| {
            let x = bounds.left();
            let y = bounds.top();
            let width = bounds.size.width;
            let height = bounds.size.height;
            let shoulder = px(6.);
            let bottom = px(14.).min(height / 2.);
            let mut p = PathBuilder::fill();
            p.move_to(point(x, y));
            p.line_to(point(x + width, y));
            p.curve_to(
                point(x + width - shoulder, y + shoulder),
                point(x + width - shoulder, y),
            );
            p.line_to(point(x + width - shoulder, y + height - bottom));
            p.curve_to(
                point(x + width - shoulder - bottom, y + height),
                point(x + width - shoulder, y + height),
            );
            p.line_to(point(x, y + height));
            p.close();
            if let Ok(path) = p.build() {
                w.paint_path(path, black());
            }
        },
    )
    .absolute()
    .size_full()
}
fn microphone(active: bool) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, w, _| {
            let origin = bounds.origin;
            let mut p = PathBuilder::stroke(px(1.4));
            let pt = |x, y| point(origin.x + px(x), origin.y + px(y));
            p.move_to(pt(5., 3.));
            p.curve_to(pt(9., 3.), pt(7., 0.));
            p.line_to(pt(9., 8.));
            p.curve_to(pt(5., 8.), pt(7., 11.));
            p.close();
            p.move_to(pt(2.5, 7.5));
            p.curve_to(pt(11.5, 7.5), pt(7., 17.));
            p.move_to(pt(7., 12.));
            p.line_to(pt(7., 15.));
            p.move_to(pt(4.5, 15.));
            p.line_to(pt(9.5, 15.));
            if let Ok(p) = p.build() {
                w.paint_path(p, if active { rgb(0xf87171) } else { rgb(0xfbbf24) });
            }
        },
    )
    .w(px(14.))
    .h(px(18.))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct IndicatorFixture {
        panel: Entity<Panel>,
    }
    impl Render for IndicatorFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .bg(rgb(0x777777))
                .child(self.panel.clone())
        }
    }
    #[::core::prelude::v1::test]
    #[ignore = "Requires Metal and VORKSPACE_DICTATION_CAPTURE_DIR; display-only fixture"]
    fn native_dictation_notch_captures() {
        use std::sync::Arc;
        let output =
            std::path::PathBuf::from(std::env::var_os("VORKSPACE_DICTATION_CAPTURE_DIR").unwrap());
        assert!(output.is_absolute() && output.is_dir());
        let states = [
            ("starting", "Starting microphone…", State::Starting),
            ("recording", "Recording", State::Recording),
            ("transcribing", "Transcribing…", State::Transcribing),
            (
                "error",
                "Microphone access is required. Use Set up permissions in Settings → General → Dictation, then allow vorkspace Clipboard in Privacy & Security.",
                State::Error,
            ),
        ];
        let selected = std::env::var("VORKSPACE_DICTATION_CAPTURE_STATE").ok();
        if selected.is_none() {
            for (name, _, _) in states {
                let result = std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "dictation_panel::tests::native_dictation_notch_captures",
                        "--ignored",
                    ])
                    .env("VORKSPACE_DICTATION_CAPTURE_STATE", name)
                    .status()
                    .unwrap();
                assert!(result.success());
            }
            return;
        }
        let (name, status, state) = states
            .into_iter()
            .find(|(name, _, _)| selected.as_deref() == Some(*name))
            .unwrap();
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
                let panel = cx.new(|_| Panel {
                    status: status.into(),
                    state,
                    placement: Placement::default(),
                    _screens: None,
                    _screen_task: None,
                });
                cx.new(|_| IndicatorFixture { panel })
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
