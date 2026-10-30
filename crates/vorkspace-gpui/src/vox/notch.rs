//! The notch is a separate, nonactivating native surface, never window chrome.
use super::*;
use crate::{clock::Instant, pane_motion::PaneSlide};
use gpui::{AnyElement, Focusable, MouseMoveEvent, PathBuilder, canvas, point};
use std::time::Duration;

pub(super) const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const DONE_TIME: Duration = Duration::from_secs(3);

pub(super) struct Presentation {
    pub geometry: native::Geometry,
    width: f32,
    height: f32,
    width_motion: Option<PaneSlide>,
    height_motion: Option<PaneSlide>,
    pub closing: bool,
    pub hovered: bool,
    pub hearing: bool,
    pub activity: String,
    pub touched: Instant,
    pub completed: Option<Instant>,
    placed: Option<(f32, f32)>,
    pub placement: native::Placement,
}

impl Default for Presentation {
    fn default() -> Self {
        let geometry = native::Geometry::default();
        Self {
            geometry,
            width: geometry.compact_width(),
            height: geometry.notch_height,
            width_motion: None,
            height_motion: None,
            closing: false,
            hovered: false,
            hearing: false,
            activity: "listening".into(),
            touched: Instant::now(),
            completed: None,
            placed: None,
            placement: native::Placement::default(),
        }
    }
}

impl Presentation {
    pub fn reposition(&mut self) {
        self.placed = None;
    }

    pub fn open(&mut self, geometry: native::Geometry) {
        self.placement.close();
        *self = Self {
            geometry,
            width: geometry.notch_width,
            height: geometry.notch_height,
            ..Default::default()
        };
        self.placement.open();
        self.width_motion = Some(PaneSlide::new(
            self.width,
            geometry.compact_width(),
            native::WING_WIDTH * 2.,
        ));
    }

    pub fn reopen(&mut self, geometry: native::Geometry) {
        self.geometry = geometry;
        self.closing = false;
        self.hearing = false;
        self.activity = "listening".into();
        self.completed = None;
        self.reposition();
        // Keep the current dimensions and velocity when a retraction reverses.
    }

    pub fn close(&mut self) {
        self.closing = true;
        self.hovered = false;
    }

    fn moving(
        value: f32,
        target: f32,
        motion: &mut Option<PaneSlide>,
        now: Instant,
    ) -> (f32, bool) {
        if motion.as_ref().is_some_and(|m| m.to != target) {
            *motion = Some(motion.as_ref().unwrap().retarget(target, now));
        } else if motion.is_none() && (value - target).abs() > 0.1 {
            *motion = Some(PaneSlide::new(value, target, target - value));
        }
        if let Some(slide) = motion {
            let (value, done) = slide.at(now);
            if done {
                *motion = None;
            }
            (value, !done)
        } else {
            (target, false)
        }
    }

    pub fn paused(&self) -> bool {
        self.hovered || self.hearing || matches!(self.activity.as_str(), "thinking" | "replying")
    }
}

impl Vox {
    pub(super) fn touch(&mut self, cx: &mut Context<Self>) {
        self.notch.touched = Instant::now();
        self.arm_idle(cx);
    }

    pub(super) fn arm_idle(&mut self, cx: &mut Context<Self>) {
        self.idle_task = None;
        if self.notch.closing || self.notch.paused() {
            return;
        }
        let generation = self.generation;
        let touched = self.notch.touched;
        let deadline = Instant::now() + IDLE_TIMEOUT;
        self.idle_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(deadline.saturating_duration_since(Instant::now()))
                .await;
            let _ = this.update_in(cx, |voice, window, cx| {
                if voice.generation != generation
                    || voice.notch.touched != touched
                    || voice.notch.closing
                    || voice.notch.paused()
                {
                    return;
                }
                if voice
                    .reply_input
                    .as_ref()
                    .is_some_and(|reply| reply.read(cx).focus_handle(cx).is_focused(window))
                {
                    voice.touch(cx);
                    return;
                }
                voice.stop(cx);
            });
        }));
    }

    pub(super) fn display_activity(&mut self, input: &Value, cx: &mut Context<Self>) {
        let activity = input["display"]["activity"].as_str().unwrap_or("listening");
        let hearing = input["display"]["hearing"] == true;
        if self.notch.activity != activity || self.notch.hearing != hearing {
            self.notch.activity = activity.into();
            self.notch.hearing = hearing;
            if hearing || activity != "listening" {
                self.notch.completed = None;
            }
            self.touch(cx);
        }
    }

    pub(super) fn completed_reply(&mut self, cx: &mut Context<Self>) {
        self.notch.completed = Some(Instant::now());
        self.touch(cx);
        let generation = self.generation;
        let completed = self.notch.completed;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DONE_TIME).await;
            let _ = this.update(cx, |voice, cx| {
                if voice.generation == generation && voice.notch.completed == completed {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn status(&self) -> &'static str {
        if self.error.is_some() {
            return "Try again";
        }
        match self.state.as_str() {
            "connecting" => "Connecting",
            "stopping" => "Saving",
            "idle" => "Done",
            _ if self.notch.hearing => "Listening",
            _ if self.notch.activity == "thinking" => "Thinking",
            _ if self.notch.activity == "replying" => {
                if self.silent {
                    "Writing"
                } else {
                    "Speaking"
                }
            }
            _ if self
                .notch
                .completed
                .is_some_and(|at| Instant::now().duration_since(at) < DONE_TIME) =>
            {
                "Done"
            }
            _ => "Listening",
        }
    }

    pub(super) fn render_notch(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let now = Instant::now();
        let message = (!self.notch.closing)
            .then(|| self.bubble_message())
            .flatten();
        let reply_height = message.as_ref().map_or(0., |(_, text)| {
            let lines = text.chars().count().div_ceil(48).clamp(2, 7);
            (lines as f32 * 20. + 44.).min(184.)
        });
        let geometry = self.notch.geometry;
        let target_width = if self.notch.closing {
            geometry.notch_width
        } else if message.is_some() {
            geometry
                .compact_width()
                .max(440.)
                .min(geometry.screen_width)
        } else {
            geometry.compact_width()
        };
        let target_height = geometry.notch_height + reply_height;
        let (width, moving_w) = Presentation::moving(
            self.notch.width,
            target_width,
            &mut self.notch.width_motion,
            now,
        );
        let (height, moving_h) = Presentation::moving(
            self.notch.height,
            target_height,
            &mut self.notch.height_motion,
            now,
        );
        self.notch.width = width;
        self.notch.height = height;
        let animated = moving_w || moving_h;
        if animated {
            window.request_animation_frame();
        }
        if let Some(panel) = self.panel
            && self.notch.placed != Some((width, height))
        {
            self.notch.placed = Some((width, height));
            cx.defer(move |cx| {
                let _ = panel.update(cx, |voice, window, cx| {
                    native::place(window, geometry, width, height, &voice.notch.placement, cx)
                });
            });
        }
        if self.notch.closing && !animated && !self.running() && self.panel.is_some() {
            let generation = self.generation;
            let voice = cx.weak_entity();
            window.defer(cx, move |window, cx| {
                let _ = voice.update(cx, |voice, _| {
                    if voice.generation == generation && voice.notch.closing && !voice.running() {
                        voice.release_panel();
                        window.remove_window();
                    }
                });
            });
        }
        let wing = ((width - geometry.notch_width) / 2.).max(0.);
        let reveal = (wing / native::WING_WIDTH).clamp(0., 1.);
        let status = self.status();
        let pulsing = !self.notch.closing
            && (self.notch.hearing || matches!(status, "Thinking" | "Connecting" | "Speaking"));
        let pulse = if pulsing {
            if !crate::clock::suppress_endless_animation_frames() {
                window.request_animation_frame();
            }
            0.55 + 0.4
                * ((now.duration_since(self.notch.touched).as_secs_f32() * 4.).sin() * 0.5 + 0.5)
        } else {
            0.8
        };
        let color = if self.error.is_some() {
            gpui::rgb(0xffb86b).into()
        } else {
            white(pulse)
        };
        let mut root = div()
            .id("vox-notch")
            .role(gpui::Role::Group)
            .aria_label(format!("Vox: {status}"))
            .debug_selector(|| "vox-notch".into())
            .w(px(width))
            .h(px(height))
            .relative()
            .overflow_hidden()
            .text_color(white(0.9))
            .font_family(crate::fonts::UI)
            .on_hover(cx.listener(|voice, hovered: &bool, _, cx| {
                voice.notch.hovered = *hovered;
                voice.touch(cx);
            }))
            .on_mouse_move(cx.listener(|voice, _: &MouseMoveEvent, _, cx| voice.touch(cx)))
            .capture_key_down(cx.listener(|voice, event: &gpui::KeyDownEvent, _, cx| {
                voice.touch(cx);
                if event.keystroke.key == "escape" {
                    voice.stop(cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                canvas(
                    |_, _, _| (),
                    |bounds, _, window, _| {
                        let x = bounds.left();
                        let y = bounds.top();
                        let w = bounds.size.width;
                        let h = bounds.size.height;
                        let top = px(6.);
                        let bottom = px(14.).min(h / 2.);
                        let mut shape = PathBuilder::fill();
                        shape.move_to(point(x, y));
                        shape.line_to(point(x + w, y));
                        shape.curve_to(point(x + w - top, y + top), point(x + w - top, y));
                        shape.line_to(point(x + w - top, y + h - bottom));
                        shape.curve_to(
                            point(x + w - top - bottom, y + h),
                            point(x + w - top, y + h),
                        );
                        shape.line_to(point(x + top + bottom, y + h));
                        shape.curve_to(point(x + top, y + h - bottom), point(x + top, y + h));
                        shape.line_to(point(x + top, y + top));
                        shape.curve_to(point(x, y), point(x + top, y));
                        shape.close();
                        if let Ok(shape) = shape.build() {
                            window.paint_path(shape, gpui::black());
                        }
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .h(px(geometry.notch_height))
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .w(px(wing))
                            .h_full()
                            .flex_shrink_0()
                            .overflow_hidden()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                crate::controls::button("vox-toggle", "Start or end Vox", true)
                                    .debug_selector(|| "vox-toggle".into())
                                    .w(px(56.))
                                    .h_full()
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .opacity(reveal)
                                    .focus_visible(|s| s.bg(white(0.15)))
                                    .font_family(crate::markdown::FIXED_FONT)
                                    .text_size(px(19.))
                                    .child(self.face.clone())
                                    .on_click(cx.listener(|voice, _, window, cx| {
                                        voice.toggle_call(window, cx)
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .debug_selector(|| "vox-notch-gap".into())
                            .w(px(geometry.notch_width))
                            .h_full()
                            .flex_shrink_0(),
                    )
                    .child(
                        div()
                            .debug_selector(|| "vox-status".into())
                            .w(px(wing))
                            .h_full()
                            .flex_shrink_0()
                            .overflow_hidden()
                            .flex()
                            .items_center()
                            .px_2()
                            .gap_1p5()
                            .opacity(reveal)
                            .child(div().size(px(5.)).rounded_full().bg(color).flex_shrink_0())
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(status),
                            )
                            .child(
                                crate::controls::icon_button("vox-stop", "End Vox call", true)
                                    .text_color(white(0.65))
                                    .hover(|s| s.bg(white(0.1)).text_color(white(1.)))
                                    .child(icon("close", 12.))
                                    .on_click(cx.listener(|voice, _, _, cx| voice.stop(cx))),
                            ),
                    ),
            );
        if let Some((_, text)) = message {
            let reply = self
                .reply_input
                .get_or_insert_with(|| {
                    cx.new(|cx| {
                        let mut input = crate::input::Input::memory_fact("Vox reply", cx);
                        input.set_read_only(true);
                        input
                    })
                })
                .clone();
            if reply.read(cx).content.as_str() != text {
                reply.update(cx, |input, cx| input.set_text(text, cx));
            }
            root = root.child(
                div()
                    .id("vox-reply")
                    .debug_selector(|| "vox-reply".into())
                    .h(px(reply_height))
                    .mx_4()
                    .pt_2()
                    .pb_3()
                    .border_t_1()
                    .border_color(white(0.1))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .id("vox-reply-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .text_size(px(13.))
                            .text_color(white(0.9))
                            .child(reply),
                    )
                    .child(
                        crate::controls::button("dismiss-vox-reply", "Dismiss Vox reply", true)
                            .focus_visible(|s| s.bg(white(0.15)))
                            .text_size(px(10.))
                            .text_color(white(0.7))
                            .child("Dismiss")
                            .on_click(cx.listener(|voice, _, _, cx| {
                                voice.dismiss_reply(cx);
                                voice.touch(cx);
                            })),
                    ),
            );
        }
        root.into_any_element()
    }
}
