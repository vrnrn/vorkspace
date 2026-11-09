//! The fluidither layer: printed surfaces, spring motion and Obel Mono.
//!
//! This module is the one owner of how vorkspace uses fluidither. Hovers stay
//! flat tints; dither marks surfaces that carry meaning. Dither is drawn
//! with the workspace theme's content ink, so it follows every theme, light and
//! dark, and the user's accent stays the only color. The cells come from the
//! GPUI fork's `dither` background: one quad per fill, clipped by its rounded
//! corners, with the same 4×4 Bayer ranks fluidither uses on the web.
use crate::theme::{black, content};
use gpui::{App, Background, Global};

pub use fluidither::generated::{GLIDE, QUICK};
pub use fluidither::spring::{LONGEST_STEP, Spring};

/// Pattern cells, in logical points. fluidither's own fine cell.
pub const CELL: f32 = fluidither::generated::DITHER_HOVER_CELL;

/// A hover wash: a flat tint of content ink. Hover stays quiet and solid; the
/// printed look belongs to surfaces that carry meaning (tracks, charts, empty
/// pages, scrims), never to the pointer passing over a row.
pub fn hover(weight: f32) -> Background {
    content(weight).into()
}

/// A hover wash in a color of its own (danger red, the accent, ink on a
/// colored fill). Flat, like `hover`.
pub fn wash(tint: gpui::Hsla) -> Background {
    tint.into()
}

/// A press: the hover tint, half again as strong.
pub fn pressed(weight: f32) -> Background {
    content((weight * 1.5).min(1.)).into()
}

/// The empty part of a slider, meter or progress track: a dotted rule. A
/// quarter tone of the Bayer matrix inks every other cell on every other row,
/// so a thin track prints as evenly spaced dots, never a checkerboard. The
/// filled part stays solid, so the value reads at a glance.
pub fn track(weight: f32) -> Background {
    gpui::dither(content((weight * 4.).min(1.)), 0.25, CELL)
}

/// The unfilled ring of a gauge, drawn as a path: the same dotted quarter
/// tone in 1pt cells, so a 2pt stroke prints as a ring of dots.
pub fn ring_track(color: gpui::Hsla) -> Background {
    let ink = gpui::Hsla {
        a: (color.a * 4.).min(1.),
        ..color
    };
    gpui::dither(ink, 0.25, 1.)
}

/// Shading under a line in an area chart: solid ink right under the line,
/// dithering out to nothing at the baseline, as fluidither's area charts do.
pub fn area(color: gpui::Hsla) -> Background {
    gpui::vertical_dither(color, 1., 0., CELL)
}

/// A bar in a bar chart: full strength at its top, printing down to three
/// quarter tone at its base. The ramp stops short of half tone, where the
/// Bayer matrix prints a checkerboard; the base reads as solid ink with holes.
pub fn bar(color: gpui::Hsla) -> Background {
    gpui::vertical_dither(color, 1., 0.75, CELL)
}

/// The printed ground under an empty state's icon: a quarter-tone dot grid
/// right beneath it that fades to paper, like a shadow cast on newsprint. It
/// never passes quarter tone, where the dots would join into a checker.
pub fn plate() -> Background {
    gpui::vertical_dither(content(0.8), 0.25, 0., CELL)
}

/// An empty state's mark: the icon on clean paper, standing above its
/// printed ground, so no dots run through the glyph. `side` is the mark's
/// width; the icon keeps its own size.
pub fn empty_mark(icon: impl gpui::IntoElement, side: f32) -> gpui::Div {
    use gpui::{ParentElement, Styled, px};
    gpui::div()
        .w(px(side))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(4.))
        .child(
            gpui::div()
                .h(px(side * 0.55))
                .flex()
                .items_center()
                .justify_center()
                .child(icon),
        )
        .child(
            gpui::div()
                .w_full()
                .h(px(side * 0.5))
                .rounded(px(3.))
                .bg(plate()),
        )
}

/// A dotted wash for drop targets and other "something can land here" hints.
pub fn hint(weight: f32) -> Background {
    gpui::dither(content((weight * 6.).min(1.)), 0.25, CELL)
}

/// `hint` in a color of its own, such as the accent for a split's landing
/// zone. `weight` is the flat wash's alpha.
pub fn hint_in(color: gpui::Hsla, weight: f32) -> Background {
    let ink = gpui::Hsla {
        a: (weight * 6.).min(1.),
        ..color
    };
    gpui::dither(ink, 0.25, CELL)
}

/// The screen behind a dialog: a flat dim at `weight`. A printed scrim
/// reads as a checkerboard over the text it covers and garbles it, so the
/// dialog's own surface carries the texture instead.
pub fn scrim(weight: f32) -> Background {
    black(weight).into()
}

/// A still, printed field for an empty page's header: fluidither's aurora
/// field in strong content ink, in large cells. It is drawn once into a
/// texture and costs nothing until its size or theme changes.
const FIELD_KIND: fluidither::fields::Field = fluidither::fields::Field::Aurora;

pub fn field(id: impl Into<gpui::ElementId>) -> impl gpui::IntoElement {
    Field(id.into())
}

/// Installs fluidither's theme on first render, so a view drawn without the
/// app's startup (a test, a detached window) still finds it.
#[derive(gpui::IntoElement)]
struct Field(gpui::ElementId);

impl gpui::RenderOnce for Field {
    fn render(self, _: &mut gpui::Window, cx: &mut App) -> impl gpui::IntoElement {
        use gpui::Styled;
        init(cx);
        fluidither::DitherGradient::field(self.0, FIELD_KIND)
            // Blue noise scatters its dots like grain: Bayer's mid-tones form
            // a checkerboard, and halftone crosses read as blocks at this size.
            .matrix(fluidither::dither::Matrix::BlueNoise)
            // Still: an empty page is not a reason to draw frames.
            .speed(0.)
            .ink(gpui::Rgba::from(content(0.3)))
            .cell(2.)
            .corner_radius(crate::radius::SURFACE)
            .w_full()
            .h_full()
    }
}

/// A keyboard shortcut as a chip: Obel Mono on a soft pill, as fluidither's
/// `Kbd`, in the workspace theme's ink. Obel Mono has no macOS key symbols
/// (⌘ ⇧ ⌥ ⌫ …), whose fallback prints at a fraction of the size, so a chip
/// carrying one uses the system font, which draws them at text size.
pub fn kbd(keys: impl Into<gpui::SharedString>) -> gpui::Div {
    use gpui::{ParentElement, Styled, px};
    let keys = keys.into();
    let font = if keys.is_ascii() {
        MONO
    } else {
        crate::fonts::UI
    };
    gpui::div()
        .flex_none()
        .min_w(px(20.))
        .flex()
        .justify_center()
        .px(px(5.))
        .rounded_full()
        .bg(content(0.07))
        .text_color(content(0.55))
        .font_family(font)
        .text_size(px(11.))
        .line_height(px(16.))
        .child(keys)
}

/// How lit the point at `x` (0 to 1 across a line of text) is by the working
/// shimmer at `seconds`, from 0 at rest to 1 at the band's centre: the sweep
/// of fluidither's shimmer, applied smoothly per glyph. Glyphs are too few
/// samples to dither, so the band is drawn as a gradient of ink instead.
pub fn shimmer_sweep(x: f32, seconds: f32) -> f32 {
    fluidither::dither::shimmer_sweep(f64::from(x), f64::from(seconds)) as f32
}

/// Keyboard hints for a picker or field: each key as a `kbd` chip beside
/// what it does, in one quiet row.
pub fn key_hints(hints: &[(&'static str, &'static str)]) -> gpui::Div {
    use gpui::{ParentElement, Styled, px};
    gpui::div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_3()
        .text_size(px(11.))
        .text_color(content(0.4))
        .children(hints.iter().map(|&(keys, what)| {
            gpui::div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(kbd(keys))
                .child(what)
        }))
}

/// A notice's tone at its leading edge, as fluidither's `Banner`: a solid
/// 3pt bar with a printed column of dots behind it. It is one quad, the bar
/// its left border and the dots its fill, so it follows the card's rounded
/// corners (`radius`, the card's own) instead of poking out of them; GPUI
/// clips children to a rectangle. Place it as the first child of a
/// `relative` card and pad the content past `STRIP_WIDTH`.
pub fn strip(tone: gpui::Hsla, radius: f32) -> gpui::Div {
    use gpui::{Styled, px};
    let ink = gpui::Hsla {
        a: tone.a * 0.6,
        ..tone
    };
    gpui::div()
        .absolute()
        .top_0()
        .bottom_0()
        .left_0()
        .w(px(STRIP_WIDTH))
        .rounded_l(px(radius))
        .border_l(px(3.))
        .border_color(tone)
        .bg(gpui::dither(ink, 0.25, CELL))
}

/// A short status label, as fluidither's `Badge`: a square cell carries the
/// tone so the label itself stays at full contrast.
pub fn badge(label: impl Into<gpui::SharedString>, tone: gpui::Hsla) -> gpui::Div {
    use gpui::{ParentElement, Styled, px};
    gpui::div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(5.))
        .h(px(20.))
        .px(px(7.))
        .rounded(px(4.))
        .bg(content(0.07))
        .text_color(content(0.9))
        .text_size(px(11.))
        .line_height(px(14.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .child(gpui::div().flex_none().size(px(6.)).bg(tone))
        .child(label.into())
}

/// Where a step of a longer job stands, for `step_mark`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Done,
    Active,
    Waiting,
    Skipped,
}

/// A step's mark, as fluidither's `Steps`: an 8pt square, solid when done,
/// outlined with printed dots while active, an empty outline while waiting,
/// and a dash when skipped. Centered in a `slot`-point square so it lines up
/// with icons of that size.
pub fn step_mark(step: Step, color: gpui::Hsla, slot: f32) -> gpui::Div {
    use gpui::{ParentElement, Styled, px};
    let mark = gpui::div().flex_none();
    let mark = match step {
        Step::Done => mark.size(px(8.)).bg(color),
        Step::Active => mark
            .size(px(8.))
            .border_1()
            .border_color(color)
            .bg(gpui::dither(color, 0.25, 1.)),
        Step::Waiting => mark.size(px(8.)).border_1().border_color(color),
        Step::Skipped => mark.w(px(8.)).h(px(1.5)).bg(color),
    };
    gpui::div()
        .flex_none()
        .size(px(slot))
        .flex()
        .items_center()
        .justify_center()
        .child(mark)
}

/// Whether a table cell reads as a number (`42`, `-3.5`, `1,204`, `85%`,
/// `$12`), so it can be set in Obel Mono with tabular digits, as
/// fluidither's tables set their numeric columns.
pub fn is_numeric(text: &str) -> bool {
    let text = text.trim();
    let text = text
        .strip_suffix('%')
        .unwrap_or(text)
        .trim_start_matches(['+', '-', '\u{2212}', '$', '\u{20ac}', '\u{a3}']);
    !text.is_empty()
        && text.starts_with(|c: char| c.is_ascii_digit() || c == '.')
        && text.chars().any(|c| c.is_ascii_digit())
        && text
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '_'))
}

/// A slider's thumb, as fluidither's: an upright rounded bar rather than a
/// dot, `THUMB_WIDTH` wide and `THUMB_HEIGHT` tall. Position it absolutely,
/// centred on the value and on the track.
pub fn thumb(color: gpui::Hsla) -> gpui::Div {
    use gpui::{Styled, px};
    gpui::div()
        .absolute()
        .w(px(THUMB_WIDTH))
        .h(px(THUMB_HEIGHT))
        .rounded(px(3.))
        .bg(color)
}

/// A compact slider's rail, track, fill and thumb, for a `relative` row
/// 14pt tall that centres its children: the dotted track, solid ink up to
/// `fraction`, and the upright thumb, kept inside the rail at both ends.
pub fn slider_parts(fraction: f32) -> [gpui::Div; 3] {
    use gpui::{Styled, px, relative};
    let fraction = fraction.clamp(0., 1.);
    [
        gpui::div()
            .w_full()
            .h(px(4.))
            .rounded_full()
            .bg(track(0.15)),
        gpui::div()
            .absolute()
            .left_0()
            .top(px(5.))
            .h(px(4.))
            .w(relative(fraction))
            .rounded_full()
            .bg(content(0.85)),
        thumb(content(1.))
            .top(px(-1.))
            .left(relative(fraction))
            .ml(px(-THUMB_WIDTH * fraction)),
    ]
}

pub const THUMB_WIDTH: f32 = 8.;
pub const THUMB_HEIGHT: f32 = 16.;

/// A switch's knob: white, as a knob reads on the solid accent and on the
/// flat off track; black only on an accent pale enough to lose it.
pub fn knob(on: bool) -> gpui::Hsla {
    if on && crate::theme::accent_color().l > 0.8 {
        black(1.)
    } else {
        gpui::white()
    }
}

/// A switch track's corners: a rounded square, as fluidither's Switch, so the
/// square knob sits in a shape of its own kind rather than inside a pill.
pub const SWITCH_RADIUS: f32 = 5.;

/// A switch's off track: a flat tint, so the knob sits on a calm field
/// instead of a checker of printed cells.
pub fn switch_off() -> gpui::Hsla {
    content(0.16)
}

/// Text and icons on a solid ink fill (a primary button, the submit): the
/// pane's paper, opaque, so it inverts with the theme.
pub fn on_ink() -> gpui::Hsla {
    let mut paper = crate::theme::pane_fill();
    paper.a = 1.;
    paper
}

/// A tooltip's fill, as fluidither's tips: solid ink, the theme inverted.
/// Opaque, so the text a tip covers never shows through it: 92% of the way
/// from the pane's paper to the ink.
pub fn tip_fill() -> gpui::Hsla {
    let ink = content(1.);
    let mut paper = crate::theme::pane_fill();
    paper.a = 1.;
    gpui::Hsla {
        h: ink.h,
        s: ink.s,
        l: paper.l + (ink.l - paper.l) * 0.92,
        a: 1.,
    }
}

/// Text on a tip: the pane's own color, so it reads as paper on ink in every
/// theme. `alpha` dims secondary lines.
pub fn tip_text(alpha: f32) -> gpui::Hsla {
    let mut paper = crate::theme::pane_fill();
    paper.a = 1.;
    gpui::Hsla { a: alpha, ..paper }
}

/// A tip's corners: a pill for one line, a soft card when it wraps.
pub const TIP_RADIUS: f32 = 14.;

/// A short tooltip: paper text on a solid ink pill.
pub fn tip() -> gpui::Div {
    use gpui::{Styled, px};
    gpui::div()
        .px(px(9.))
        .py(px(4.))
        .rounded(px(TIP_RADIUS))
        .bg(tip_fill())
        .text_color(tip_text(1.))
        .text_size(px(11.))
        .line_height(px(16.))
}

/// How far `strip` reaches into a card; content starts past it.
pub const STRIP_WIDTH: f32 = 16.;

/// A horizontal rule printed as a line of dots, for Markdown's `---`.
pub fn rule() -> gpui::Div {
    use gpui::{Styled, px};
    gpui::div()
        .w_full()
        .h(px(2.))
        .bg(gpui::dither(content(0.7), 0.25, 1.))
}

/// A whole number that rolls to each new value, as fluidither's `Count`.
/// It shows its first value at once; later changes count up or down on the
/// `QUICK` spring.
#[derive(gpui::IntoElement)]
pub struct Count {
    id: gpui::ElementId,
    value: i64,
    format: fn(i64) -> String,
}

impl Count {
    pub fn new(id: impl Into<gpui::ElementId>, value: i64, format: fn(i64) -> String) -> Self {
        Self {
            id: id.into(),
            value,
            format,
        }
    }
}

struct Rolling {
    spring: Spring,
    last: crate::clock::Instant,
}

impl gpui::RenderOnce for Count {
    fn render(self, window: &mut gpui::Window, cx: &mut App) -> impl gpui::IntoElement {
        use gpui::ParentElement;
        let value = self.value;
        let reduce_motion = cx.reduce_motion();
        let state = window.use_keyed_state(self.id, cx, |_, _| Rolling {
            spring: Spring::new(QUICK, value as f64),
            last: crate::clock::Instant::now(),
        });
        let (shown, landed) = state.update(cx, |rolling, _| {
            if reduce_motion {
                rolling.spring = Spring::new(QUICK, value as f64);
                return (value, true);
            }
            if rolling.spring.target != value as f64 {
                rolling.spring.target = value as f64;
            }
            let landed = step(&mut rolling.spring, &mut rolling.last);
            (rolling.spring.position.round() as i64, landed)
        });
        if !landed {
            window.request_animation_frame();
        }
        gpui::div().child((self.format)(if landed { value } else { shown }))
    }
}

/// Code, keyboard hints and numeric readouts. Prose and controls stay on the
/// system font; the terminal and editor keep the user's fixed font.
pub const MONO: &str = fluidither::generated::FONT_MONO;

struct Loaded;
impl Global for Loaded {}

/// Loads Obel Mono once per app. Safe to call from every window's root.
pub fn init(cx: &mut App) {
    if cx.has_global::<Loaded>() {
        return;
    }
    fluidither::load_fonts(cx);
    // fluidither's components read its theme global. Vorkspace passes its own
    // ink to every one it draws and keeps Tab focus to itself, so only the
    // global is installed, not `fluidither::init`'s key bindings.
    cx.set_global(fluidither::Theme {
        mode: if crate::theme::is_light() {
            fluidither::Mode::Light
        } else {
            fluidither::Mode::Dark
        },
        mono_font: MONO.into(),
        reduced_motion: false,
        keyboard_input: false,
        modal: None,
    });
    cx.set_global(Loaded);
}

/// Steps a spring by the time since `last`, which it then resets. Returns
/// whether it has landed, so the caller knows to request another frame.
pub fn step(spring: &mut Spring, last: &mut crate::clock::Instant) -> bool {
    let now = crate::clock::Instant::now();
    let dt = now
        .saturating_duration_since(*last)
        .as_secs_f64()
        .min(LONGEST_STEP);
    *last = now;
    spring.step(dt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hovers_are_flat_tints() {
        for weight in [0.05_f32, 0.08, 0.1] {
            assert_eq!(hover(weight).as_solid(), Some(content(weight)));
        }
        assert!(track(0.1).as_solid().is_none());
    }

    #[test]
    fn numbers_are_recognised_and_words_are_not() {
        for number in ["42", "-3.5", "1,204", "85%", "$12", "+7", ".5", "\u{2212}2"] {
            assert!(is_numeric(number), "{number}");
        }
        for word in ["", "Mon", "v1.2", "1.2.3a", "%", "-", "12 ms", "N/A"] {
            assert!(!is_numeric(word), "{word}");
        }
    }

    #[test]
    fn spring_lands_on_its_target() {
        let mut spring = Spring::new(GLIDE, 0.);
        spring.target = 100.;
        let mut landed = false;
        for _ in 0..120 {
            landed = spring.step(1. / 60.);
            if landed {
                break;
            }
        }
        assert!(landed);
        assert_eq!(spring.position, 100.);
    }
}
