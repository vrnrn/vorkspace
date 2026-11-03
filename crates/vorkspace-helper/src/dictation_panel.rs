//! A quiet extension of the hardware notch; opening never takes keyboard focus.
use gpui::{prelude::*, *};
use vorkspace_helper::{DictationPhase, DictationStatus};
use vorkspace_macos::notch::{self, Geometry, Placement};

pub struct Panel {
    pub status: DictationStatus,
    pub level: f32,
    pub placement: Placement,
    _screens: Option<notch::ScreenChanges>,
    _screen_task: Option<Task<()>>,
    #[cfg(test)]
    geometry: Option<Geometry>,
}
pub fn open(status: DictationStatus, cx: &mut App) -> Result<WindowHandle<Panel>> {
    cx.open_window(notch::window_options(Geometry::read()), |_, cx| {
        cx.new(|cx| {
            let (watch, receiver) = notch::ScreenChanges::new();
            let task = cx.spawn(async move |this, cx| {
                while receiver.recv().await.is_ok() {
                    let _ = this.update(cx, |_, cx| cx.notify());
                }
            });
            let placement = Placement::default();
            placement.open();
            Panel {
                status,
                level: 0.,
                placement,
                _screens: Some(watch),
                _screen_task: Some(task),
                #[cfg(test)]
                geometry: None,
            }
        })
    })
}

impl Render for Panel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        let geometry = self.geometry.unwrap_or_else(Geometry::read);
        #[cfg(not(test))]
        let geometry = Geometry::read();
        let listening = self.status.phase == DictationPhase::Listening;
        let error = !self.status.active;
        let attached = geometry.notch_edges.is_some();
        let width = if error {
            420.
        } else if attached {
            geometry.compact_width()
        } else {
            184.
        };
        let height = geometry.notch_height + if error { 56. } else { 0. };
        notch::place(window, geometry, width, height, &self.placement, cx);
        let label = match self.status.phase {
            DictationPhase::Listening => "Listening",
            DictationPhase::Starting => "Starting…",
            DictationPhase::Finishing => "Finishing…",
            DictationPhase::Cancelling => "Cancelling…",
            _ => "Dictation",
        };
        div()
            .id("dictation-notch")
            .debug_selector(|| "dictation-notch".into())
            .role(Role::Group)
            .aria_label(format!("Dictation: {}", self.status.message))
            .relative()
            .w(px(width))
            .h(px(height))
            .overflow_hidden()
            .font_family(vorkspace_ui::fonts::UI)
            .text_size(px(12.))
            .text_color(white())
            .child(notch::body())
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(geometry.notch_height))
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(10.))
                    .child(
                        action(
                            "dictation-finish",
                            if listening {
                                "Finish dictation"
                            } else {
                                "Cancel dictation"
                            },
                        )
                        .child(microphone(listening))
                        .on_click(move |_, _, cx| {
                            if listening {
                                super::dictation::toggle(cx)
                            } else {
                                super::dictation::cancel(cx)
                            }
                        }),
                    )
                    .when(!attached, |d| d.child(div().child(label)))
                    .when(attached, |d| d.child(div().flex_1()))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .when(listening, |d| d.child(waveform(self.level)))
                            .child(
                                action("dictation-cancel", "Cancel dictation (Escape)")
                                    .child(close_icon())
                                    .on_click(|_, _, cx| super::dictation::cancel(cx)),
                            ),
                    ),
            )
            .when(error, |d| {
                d.child(
                    div()
                        .debug_selector(|| "dictation-error".into())
                        .relative()
                        .px(px(20.))
                        .pt(px(4.))
                        .w_full()
                        .max_h(px(48.))
                        .text_color(rgba(0xffffffdd))
                        .child(self.status.message.clone()),
                )
            })
    }
}

fn action(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .size(px(24.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .hover(|s| s.bg(rgba(0xffffff18)))
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
}
fn microphone(active: bool) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let p = |x, y| point(bounds.left() + px(x), bounds.top() + px(y));
            let mut path = PathBuilder::stroke(px(1.5));
            path.move_to(p(5., 3.));
            path.curve_to(p(9., 3.), p(7., 0.));
            path.line_to(p(9., 8.));
            path.curve_to(p(5., 8.), p(7., 11.));
            path.close();
            path.move_to(p(2.5, 7.5));
            path.curve_to(p(11.5, 7.5), p(7., 17.));
            path.move_to(p(7., 12.));
            path.line_to(p(7., 15.));
            path.move_to(p(4.5, 15.));
            path.line_to(p(9.5, 15.));
            if let Ok(path) = path.build() {
                window.paint_path(path, if active { rgb(0xf87171) } else { rgb(0xffffff) });
            }
        },
    )
    .w(px(14.))
    .h(px(18.))
}
fn close_icon() -> impl IntoElement {
    canvas(
        |_, _, _| (),
        |bounds, _, window, _| {
            let mut path = PathBuilder::stroke(px(1.3));
            path.move_to(bounds.origin + point(px(2.), px(2.)));
            path.line_to(bounds.origin + point(px(10.), px(10.)));
            path.move_to(bounds.origin + point(px(10.), px(2.)));
            path.line_to(bounds.origin + point(px(2.), px(10.)));
            if let Ok(path) = path.build() {
                window.paint_path(path, rgba(0xffffffbb));
            }
        },
    )
    .size(px(12.))
}
fn waveform(level: f32) -> impl IntoElement {
    div()
        .id("dictation-level")
        .role(Role::Image)
        .aria_label("Microphone input level")
        .h(px(20.))
        .flex()
        .items_center()
        .gap(px(2.))
        .children([0.45, 0.8, 1., 0.65].into_iter().map(move |scale| {
            div()
                .w(px(2.))
                .h(px(2. + 16. * level * scale))
                .rounded_full()
                .bg(white())
        }))
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
    #[ignore = "Requires Metal and VORKSPACE_DICTATION_CAPTURE_DIR; isolated display fixture"]
    fn dictation_notch_captures() {
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
        let panel = cx.new(|_| Panel {
            status: DictationStatus::default(),
            level: 0.65,
            placement: Placement::default(),
            _screens: None,
            _screen_task: None,
            geometry: Some(Geometry::default()),
        });
        let window = cx
            .open_window(size(px(480.), px(140.)), |_, cx| {
                cx.new(|_| Fixture(panel.clone()))
            })
            .unwrap();
        for (mode, attached) in [("notched", true), ("fallback", false)] {
            for (name, phase, message, active) in [
                (
                    "starting",
                    DictationPhase::Starting,
                    "Starting microphone…",
                    true,
                ),
                ("listening", DictationPhase::Listening, "Listening", true),
                ("finishing", DictationPhase::Finishing, "Finishing…", true),
                (
                    "error",
                    DictationPhase::Error,
                    "Allow Accessibility for vorkspace helper to type into other apps.",
                    false,
                ),
            ] {
                panel.update(&mut cx, |p, cx| {
                    p.status = DictationStatus {
                        phase,
                        message: message.into(),
                        active,
                    };
                    p.geometry = Some(Geometry {
                        notch_width: 185.,
                        notch_height: 32.,
                        notch_edges: attached.then_some((620., 820.)),
                        ..Default::default()
                    });
                    cx.notify();
                });
                cx.run_until_parked();
                vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(output.join(format!("notch-{mode}-{name}.png")))
                    .unwrap();
            }
        }
    }
}
