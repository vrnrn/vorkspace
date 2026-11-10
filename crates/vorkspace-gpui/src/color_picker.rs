//! ColorPickerPopover.tsx's saturation/value surface, hue track and hex field.
use crate::{
    input::{self, Input},
    popover::AnchorSlot,
};
use gpui::{
    Context, Entity, EventEmitter, MouseButton, Role, Subscription, Window, div, prelude::*, px,
    rgb,
};
use gpui::{DispatchPhase, canvas, linear_color_stop, linear_gradient};
use vorkspace_ui::radius;
use vorkspace_ui::theme::*;

pub(crate) struct Picked(pub u32);
#[derive(Clone, Copy)]
struct Hsv {
    h: f32,
    s: f32,
    v: f32,
}
impl Hsv {
    fn from_hex(color: u32) -> Self {
        let (r, g, b) = (
            ((color >> 16) & 255) as f32 / 255.,
            ((color >> 8) & 255) as f32 / 255.,
            (color & 255) as f32 / 255.,
        );
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;
        let h = if delta == 0. {
            0.
        } else if max == r {
            ((g - b) / delta).rem_euclid(6.) / 6.
        } else if max == g {
            ((b - r) / delta + 2.) / 6.
        } else {
            ((r - g) / delta + 4.) / 6.
        };
        Self {
            h,
            s: if max == 0. { 0. } else { delta / max },
            v: max,
        }
    }
    fn hex(self) -> u32 {
        let c = self.v * self.s;
        let h = self.h * 6.;
        let x = c * (1. - (h.rem_euclid(2.) - 1.).abs());
        let m = self.v - c;
        let (r, g, b) = match h as usize {
            0 => (c, x, 0.),
            1 => (x, c, 0.),
            2 => (0., c, x),
            3 => (0., x, c),
            4 => (x, 0., c),
            _ => (c, 0., x),
        };
        (((r + m) * 255.).round() as u32) << 16
            | (((g + m) * 255.).round() as u32) << 8
            | ((b + m) * 255.).round() as u32
    }
}
pub(crate) struct ColorPicker {
    hsv: Hsv,
    hex: Entity<Input>,
    sv: AnchorSlot,
    hue: AnchorSlot,
    drag: Option<ColorControl>,
    _change: Subscription,
}
#[derive(Clone, Copy)]
enum ColorControl {
    Hue,
    SaturationBrightness,
}
impl EventEmitter<Picked> for ColorPicker {}
impl ColorPicker {
    pub(crate) fn new(color: u32, cx: &mut Context<Self>) -> Self {
        let hex = cx.new(|cx| {
            let mut field = Input::field("Hex color", "#rrggbb", content(0.35), cx);
            field.set_font_size(12.);
            field.set_line_height(18.);
            field.set_text(format!("#{color:06x}"), cx);
            field
        });
        let change = cx.subscribe(&hex, |this, field, _: &input::Changed, cx| {
            let text = field.read(cx).content.trim();
            if text.len() == 7
                && text.starts_with('#')
                && let Ok(color) = u32::from_str_radix(&text[1..], 16)
                && color != this.hsv.hex()
            {
                this.hsv = Hsv::from_hex(color);
                cx.emit(Picked(color));
                cx.notify();
            }
        });
        Self {
            hsv: Hsv::from_hex(color),
            hex,
            sv: AnchorSlot::default(),
            hue: AnchorSlot::default(),
            drag: None,
            _change: change,
        }
    }
    pub(crate) fn set_color(&mut self, color: u32, cx: &mut Context<Self>) {
        if self.hsv.hex() != color {
            self.set_hsv(Hsv::from_hex(color), cx);
        }
    }
    fn set_hsv(&mut self, hsv: Hsv, cx: &mut Context<Self>) {
        let previous = self.hsv.hex();
        self.hsv = hsv;
        let color = hsv.hex();
        if previous != color {
            let text = format!("#{color:06x}");
            self.hex.update(cx, |field, cx| field.set_text(text, cx));
            cx.emit(Picked(color));
        }
        cx.notify();
    }
    fn pick(
        &mut self,
        control: ColorControl,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let anchor = match control {
            ColorControl::Hue => &self.hue,
            ColorControl::SaturationBrightness => &self.sv,
        };
        let Some(bounds) = anchor.get() else {
            return;
        };
        if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
            return;
        }
        let x = ((position.x - bounds.left()) / bounds.size.width).clamp(0., 1.);
        let mut hsv = self.hsv;
        match control {
            ColorControl::Hue => hsv.h = x.min(0.99999),
            ColorControl::SaturationBrightness => {
                hsv.s = x;
                hsv.v = 1. - ((position.y - bounds.top()) / bounds.size.height).clamp(0., 1.);
            }
        }
        self.set_hsv(hsv, cx);
    }
    fn nudge(&mut self, control: ColorControl, key: &str, cx: &mut Context<Self>) {
        let mut hsv = self.hsv;
        let step = 0.01;
        match (control, key) {
            (ColorControl::Hue, "left" | "down") => hsv.h = (hsv.h - step).max(0.),
            (ColorControl::Hue, "right" | "up") => hsv.h = (hsv.h + step).min(0.99999),
            (ColorControl::SaturationBrightness, "left") => hsv.s = (hsv.s - step).max(0.),
            (ColorControl::SaturationBrightness, "right") => hsv.s = (hsv.s + step).min(1.),
            (ColorControl::SaturationBrightness, "down") => hsv.v = (hsv.v - step).max(0.),
            (ColorControl::SaturationBrightness, "up") => hsv.v = (hsv.v + step).min(1.),
            _ => return,
        }
        self.set_hsv(hsv, cx);
        cx.stop_propagation();
    }
}
impl gpui::Render for ColorPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.weak_entity();
        let hue = Hsv {
            h: self.hsv.h,
            s: 1.,
            v: 1.,
        }
        .hex();
        let color = self.hsv.hex();
        let knob = || {
            div()
                .absolute()
                .size(px(14.))
                .rounded_full()
                .border_2()
                .border_color(gpui::white())
        };
        div()
            .id("bot-custom-color-picker")
            .debug_selector(|| "bot-custom-color-picker".into())
            .relative()
            .mt_2()
            .rounded(px(radius::CONTAINER))
            .border_1()
            .border_color(content(0.1))
            .bg(content(0.05))
            .p_2()
            .child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        let moving = weak.clone();
                        window.on_mouse_event(move |event: &gpui::MouseMoveEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble {
                                let _ = moving.update(cx, |picker, cx| {
                                    if event.pressed_button != Some(MouseButton::Left) {
                                        picker.drag = None;
                                    } else if let Some(control) = picker.drag {
                                        picker.pick(control, event.position, cx);
                                    }
                                });
                            }
                        });
                        let released = weak.clone();
                        window.on_mouse_event(move |_: &gpui::MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble {
                                let _ = released.update(cx, |picker, _| picker.drag = None);
                            }
                        });
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .id("bot-color-sv")
                    .role(Role::Slider)
                    .map(tab_reachable)
                    .aria_label("Saturation and brightness")
                    .aria_value(format!(
                        "{:.0}% saturation, {:.0}% brightness",
                        self.hsv.s * 100.,
                        self.hsv.v * 100.,
                    ))
                    .aria_description(
                        "Left and right change saturation. Up and down change brightness.",
                    )
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                        this.nudge(ColorControl::SaturationBrightness, &event.keystroke.key, cx);
                    }))
                    .relative()
                    .h(px(112.))
                    .w_full()
                    .rounded(px(radius::CONTROL))
                    .bg(linear_gradient(
                        90.,
                        linear_color_stop(gpui::white(), 0.),
                        linear_color_stop(rgb(hue), 1.),
                    ))
                    .child(self.sv.probe())
                    .child(div().absolute().inset_0().rounded(px(radius::CONTROL)).bg(
                        linear_gradient(
                            180.,
                            linear_color_stop(gpui::transparent_black(), 0.),
                            linear_color_stop(gpui::black(), 1.),
                        ),
                    ))
                    .child(
                        knob()
                            .left(gpui::relative(self.hsv.s))
                            .top(gpui::relative(1. - self.hsv.v))
                            .ml(px(-7.))
                            .mt(px(-7.))
                            .bg(rgb(color)),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                            this.drag = Some(ColorControl::SaturationBrightness);
                            this.pick(ColorControl::SaturationBrightness, event.position, cx);
                            cx.stop_propagation();
                        }),
                    ),
            )
            .child(
                div()
                    .id("bot-color-hue")
                    .role(Role::Slider)
                    .map(tab_reachable)
                    .aria_label("Hue")
                    .aria_numeric_value(f64::from(self.hsv.h) * 360.)
                    .aria_min_numeric_value(0.)
                    .aria_max_numeric_value(360.)
                    .aria_numeric_value_step(3.6)
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                        this.nudge(ColorControl::Hue, &event.keystroke.key, cx);
                    }))
                    .relative()
                    .mt_2()
                    .h(px(12.))
                    .w_full()
                    .child(self.hue.probe())
                    .child(
                        div()
                            .size_full()
                            .rounded_full()
                            .overflow_hidden()
                            .flex()
                            .children((0..6).map(|i| {
                                div().flex_1().h_full().bg(linear_gradient(
                                    90.,
                                    linear_color_stop(
                                        rgb(Hsv {
                                            h: i as f32 / 6.,
                                            s: 1.,
                                            v: 1.,
                                        }
                                        .hex()),
                                        0.,
                                    ),
                                    linear_color_stop(
                                        rgb(Hsv {
                                            h: (i + 1) as f32 / 6.,
                                            s: 1.,
                                            v: 1.,
                                        }
                                        .hex()),
                                        1.,
                                    ),
                                ))
                            })),
                    )
                    .child(
                        knob()
                            .top(px(-1.))
                            .left(gpui::relative(self.hsv.h))
                            .ml(px(-7.))
                            .bg(rgb(hue)),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                            this.drag = Some(ColorControl::Hue);
                            this.pick(ColorControl::Hue, event.position, cx);
                            cx.stop_propagation();
                        }),
                    ),
            )
            .child(
                div()
                    .mt_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .size(px(28.))
                            .flex_shrink_0()
                            .rounded(px(radius::CONTROL))
                            .border_1()
                            .border_color(content(0.1))
                            .bg(rgb(color)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .px_2()
                            .py_1()
                            .rounded(px(radius::CONTROL))
                            .border_1()
                            .border_color(content(0.1))
                            .bg(content(0.05))
                            .font_family(vorkspace_ui::fonts::MONO)
                            .child(self.hex.clone()),
                    ),
            )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[gpui::test]
    fn mirrored_color_changes_settle_without_rewriting_the_hex_field(
        cx: &mut gpui::TestAppContext,
    ) {
        let picker = cx.new(|cx| ColorPicker::new(0xff0000, cx));
        let picks = std::rc::Rc::new(std::cell::Cell::new(0));
        let count = picks.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&picker, move |picker, event: &Picked, cx| {
                let next = count.get() + 1;
                // Fail promptly if the old feedback loop comes back.
                assert!(next <= 2, "a mirrored color change must settle");
                count.set(next);
                picker.update(cx, |picker, cx| picker.set_color(event.0, cx));
            })
        });
        picker.update(cx, |picker, cx| picker.set_color(0x459cf7, cx));
        assert_eq!(picks.get(), 1);
        picker.update(cx, |picker, cx| {
            assert_eq!(picker.hex.read(cx).content.as_ref(), "#459cf7");
            picker.set_color(0x459cf7, cx);
        });
        assert_eq!(picks.get(), 1);
        picker.update(cx, |picker, cx| {
            picker
                .hex
                .update(cx, |field, cx| field.set_text("#37beb4".into(), cx));
        });
        assert_eq!(picks.get(), 2);
        assert_eq!(picker.read_with(cx, |picker, _| picker.hsv.hex()), 0x37beb4);
    }

    #[test]
    fn hue_saturation_and_brightness_preserve_hex_colors() {
        for color in [
            0, 0xffffff, 0xff0000, 0xff8800, 0x459cf7, 0xea603e, 0x37beb4, 0x8b63e9,
        ] {
            assert_eq!(Hsv::from_hex(color).hex(), color);
        }
    }

    #[gpui::test]
    fn changing_hue_in_gray_survives_the_parent_mirroring_the_color(cx: &mut gpui::TestAppContext) {
        let picker = cx.new(|cx| ColorPicker::new(0x808080, cx));
        picker.update(cx, |picker, cx| {
            for _ in 0..60 {
                picker.nudge(ColorControl::Hue, "right", cx);
            }
            picker.set_color(0x808080, cx);
            assert_eq!(picker.hsv.hex(), 0x808080);
            for _ in 0..50 {
                picker.nudge(ColorControl::SaturationBrightness, "right", cx);
            }
            let color = picker.hsv.hex();
            assert!(color & 0xff > (color >> 16) & 0xff);
            assert_eq!(
                picker.hex.read(cx).content.as_ref(),
                format!("#{color:06x}")
            );
        });
    }
}
