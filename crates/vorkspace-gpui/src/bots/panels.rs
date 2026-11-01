mod overview;
mod settings_modal;
use super::*;
use crate::{
    pickers::{ModelList, ModelPicked},
    popover::{MenuAction, MenuItem, MenuStyle, Side},
    project_settings::{BackgroundDialog, Event as BackgroundEvent},
};
use gpui::{AnyView, Div, FontWeight, Stateful};
use vorkspace_core::project_appearance::{
    ProjectAppearance, TEXTURE_PALETTES, default_foreground_color,
};
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum OverviewTab {
    Sessions,
    Habits,
    Documents,
    Changes,
    Todos,
}
#[derive(Clone, Copy, PartialEq)]
pub(super) enum SettingsSection {
    General,
    Soul,
    Memory,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Route {
    Review,
    Session(i64),
    Document(String),
    Habit(String),
    NewHabit,
}
impl Route {
    pub(super) fn key(&self) -> String {
        match self {
            Self::Review => "review".into(),
            Self::Session(id) => format!("session:{id}"),
            Self::Document(id) => format!("document:{id}"),
            Self::Habit(id) => format!("habit:{id}"),
            Self::NewHabit => "new-habit".into(),
        }
    }
}
pub(super) struct OpenPicker {
    pub(super) menu: Entity<Menu>,
    pub(super) anchor: Anchor,
    pub(super) width: f32,
    pub(super) align: crate::popover::Align,
    pub(super) _events: Vec<Subscription>,
}
pub(super) fn button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    let label = label.into();
    crate::controls::button(id, label.clone(), true)
        .cursor(gpui::CursorStyle::Arrow)
        .flex()
        .items_center()
        .gap_1p5()
        .px_2p5()
        .py_1p5()
        .rounded_md()
        .text_size(px(12.))
        .text_color(content(0.8))
        .hover(|s| s.bg(content(0.08)).text_color(content(1.)))
        .child(label)
}
fn setting(label: &str, description: Option<&str>, control: AnyElement) -> Div {
    div()
        .min_h(px(48.))
        .flex()
        .items_center()
        .gap_4()
        .px_3()
        .py_2()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(13.))
                        .line_height(px(20.))
                        .text_color(content(0.9))
                        .child(label.to_string()),
                )
                .when_some(description, |d, text| {
                    d.child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(20.))
                            .text_color(content(0.4))
                            .child(text.to_string()),
                    )
                }),
        )
        .child(
            div()
                .w(px(224.))
                .max_w(gpui::relative(0.6))
                .min_w_0()
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_end()
                .pr_1p5()
                .text_size(px(12.))
                .text_color(content(0.85))
                .child(control),
        )
}
fn switch_setting(label: &str, description: &str, disabled: bool, control: AnyElement) -> Div {
    div()
        .flex()
        .items_center()
        .gap_3()
        .px_3()
        .py_2()
        .when(disabled, |d| d.opacity(0.4))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(13.))
                        .line_height(px(20.))
                        .text_color(content(0.9))
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(20.))
                        .text_color(content(0.4))
                        .truncate()
                        .child(description.to_string()),
                ),
        )
        .child(control)
}
fn heading(title: &str, description: Option<&str>) -> Div {
    div()
        .pt_7()
        .pb_2()
        .px_1()
        .text_size(px(12.))
        .text_color(content(0.7))
        .font_weight(FontWeight::MEDIUM)
        .child(title.to_string())
        .when_some(description, |d, text| {
            d.child(
                div()
                    .line_height(px(20.))
                    .text_color(content(0.4))
                    .font_weight(FontWeight::NORMAL)
                    .child(text.to_string()),
            )
        })
}
fn card() -> Div {
    div()
        .rounded_xl()
        .border_1()
        .border_color(content(0.04))
        .overflow_hidden()
        .bg(content(0.02))
        .flex()
        .flex_col()
}
/// botPanelParts' EmptyState: an icon over a short, centred note.
fn empty_state(glyph: &'static str, message: &str) -> AnyElement {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_3()
        .px_6()
        .py_10()
        .text_center()
        .child(icon(glyph, 18.).text_color(content(0.55)))
        .child(
            div()
                .max_w(px(288.))
                .text_size(px(12.))
                .line_height(px(18.))
                .text_color(content(0.55))
                .child(message.to_string()),
        )
        .into_any_element()
}
fn empty(message: &str) -> AnyElement {
    div()
        .px_5()
        .py_8()
        .text_size(px(12.))
        .line_height(px(20.))
        .text_color(content(0.45))
        .child(message.to_string())
        .into_any_element()
}
struct LookChanged {
    bot: Bot,
}
/// Enter in the name field: BotMenu commits the name and closes.
struct LookSubmitted;
#[derive(Clone, Copy, PartialEq, Eq)]
enum EditingColor {
    Background,
    Foreground,
}
struct LookEditor {
    bot: Bot,
    name: Entity<Input>,
    settings: bool,
    shape_open: bool,
    dragging_grid: bool,
    grid_bounds: AnchorSlot,
    editing_color: Option<EditingColor>,
    custom: Entity<super::color_picker::ColorPicker>,
    focus: gpui::FocusHandle,
    _events: Vec<Subscription>,
}
impl EventEmitter<LookChanged> for LookEditor {}
impl EventEmitter<LookSubmitted> for LookEditor {}
impl LookEditor {
    fn new(bot: Bot, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| {
            let mut f = Input::field("Bot name", bot.label(), content(0.45), cx);
            f.set_text(bot.name.clone(), cx);
            f
        });
        let custom = cx.new(|cx| super::color_picker::ColorPicker::new(bot.color, cx));
        let events = vec![
            cx.subscribe(
                &custom,
                |this, _, event: &super::color_picker::Picked, cx| {
                    match this.editing_color.unwrap_or(EditingColor::Background) {
                        EditingColor::Background => this.bot.color = event.0,
                        EditingColor::Foreground => this.bot.fg_color = Some(event.0),
                    }
                    cx.emit(LookChanged {
                        bot: this.bot.clone(),
                    });
                    cx.notify();
                },
            ),
            cx.subscribe(&name, |this, input, _: &input::Changed, cx| {
                this.bot.name = input.read(cx).content.chars().take(40).collect();
                cx.emit(LookChanged {
                    bot: this.bot.clone(),
                });
            }),
            cx.subscribe(&name, |_, _, _: &input::Submitted, cx| {
                cx.emit(LookSubmitted)
            }),
        ];
        Self {
            bot,
            name,
            settings: false,
            shape_open: false,
            dragging_grid: false,
            grid_bounds: AnchorSlot::default(),
            editing_color: None,
            custom,
            focus: cx.focus_handle(),
            _events: events,
        }
    }
    fn randomize_seed(&mut self, cx: &mut Context<Self>) {
        let next = 10000
            + ((vorkspace_core::time::now_ms() as u32)
                .wrapping_mul(1664525)
                .wrapping_add(self.bot.resolved_seed())
                % 90000);
        self.bot.seed = Some(next);
        cx.emit(LookChanged {
            bot: self.bot.clone(),
        });
        cx.notify();
    }
    fn set_grid_from_x(&mut self, x: gpui::Pixels, cx: &mut Context<Self>) {
        let Some(bounds) = self.grid_bounds.get() else {
            return;
        };
        let width = (bounds.size.width.as_f32() - 12.).max(1.);
        let t = ((x - bounds.left()).as_f32() - 6.) / width;
        let next = (2. + t.clamp(0., 1.) * 6.).round() as u8;
        if self.bot.resolved_grid() != next {
            self.bot.grid = Some(next);
            cx.emit(LookChanged {
                bot: self.bot.clone(),
            });
            cx.notify();
        }
    }
    fn toggle_color_picker(&mut self, target: EditingColor, cx: &mut Context<Self>) {
        if self.editing_color == Some(target) {
            self.editing_color = None;
        } else {
            self.editing_color = Some(target);
            let c = match target {
                EditingColor::Background => self.bot.color,
                EditingColor::Foreground => self.bot.resolved_fg_color(),
            };
            self.custom.update(cx, |picker, cx| picker.set_color(c, cx));
        }
        cx.notify();
    }
    fn shape_section(&self, prefix: &'static str, cx: &mut Context<Self>) -> AnyElement {
        const SHAPE_LABELS: [(&str, &str); 5] = [
            ("squircle", "Squircle"),
            ("circle", "Circle"),
            ("rounded", "Rounded"),
            ("pill", "Capsule"),
            ("hex", "Soft Hex"),
        ];
        let current_shape = self.bot.badge_shape();
        let current_label = SHAPE_LABELS
            .iter()
            .find(|(id, _)| *id == current_shape)
            .map(|(_, l)| *l)
            .unwrap_or("Squircle");
        let dropdown_id = format!("{prefix}-shape-dropdown");
        let selector_dropdown = dropdown_id.clone();
        div()
            .px_1()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(content(0.75))
                            .child("Shape"),
                    )
                    .child(
                        div()
                            .id(SharedString::from(dropdown_id))
                            .debug_selector(move || selector_dropdown.clone())
                            .role(Role::ComboBox)
                            .aria_label("Shape")
                            .aria_expanded(self.shape_open)
                            .h(px(26.))
                            .px_2p5()
                            .rounded_md()
                            .border_1()
                            .border_color(content(0.14))
                            .bg(content(0.05))
                            .hover(|s| s.bg(content(0.1)).border_color(content(0.24)))
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(12.))
                            .text_color(content(0.9))
                            .child(current_label)
                            .child(icon("chevron-down", 12.).text_color(content(0.5)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.shape_open = !this.shape_open;
                                cx.notify();
                            })),
                    ),
            )
            .when(self.shape_open, |d| {
                d.child(
                    div()
                        .id(SharedString::from(format!("{prefix}-shape-options")))
                        .debug_selector(move || format!("{prefix}-shape-options"))
                        .role(Role::ListBox)
                        .aria_label("Badge shape")
                        .p_1()
                        .rounded_md()
                        .border_1()
                        .border_color(content(0.12))
                        .bg(content(0.06))
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .children(SHAPE_LABELS.iter().enumerate().map(
                            |(ix, (shape_id, label))| {
                                let selected = current_shape == *shape_id;
                                let opt_id = format!("{prefix}-shape-{ix}");
                                let opt_sel = opt_id.clone();
                                div()
                                    .id(SharedString::from(opt_id))
                                    .debug_selector(move || opt_sel.clone())
                                    .role(Role::ListBoxOption)
                                    .aria_label(*label)
                                    .aria_selected(selected)
                                    .h(px(26.))
                                    .px_2()
                                    .rounded(px(4.))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .text_size(px(12.))
                                    .text_color(content(if selected { 1. } else { 0.75 }))
                                    .when(selected, |d| d.bg(content(0.12)))
                                    .hover(|s| s.bg(content(0.08)).text_color(content(1.)))
                                    .child(*label)
                                    .when(selected, |d| {
                                        d.child(icon("check", 12.).text_color(content(1.)))
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.bot.shape = (*shape_id).into();
                                        this.shape_open = false;
                                        cx.emit(LookChanged {
                                            bot: this.bot.clone(),
                                        });
                                        cx.notify();
                                    }))
                            },
                        )),
                )
            })
            .into_any_element()
    }
    fn texture_and_grid_section(&self, prefix: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let textured = self.bot.textured;
        let grid = self.bot.resolved_grid();
        let fraction = (grid.saturating_sub(2) as f32 / 6.).clamp(0., 1.);
        let probe = self.grid_bounds.clone();
        let toggle_id = format!("{prefix}-texture-toggle");
        let toggle_sel = toggle_id.clone();
        let rand_id = format!("{prefix}-randomize-seed");
        let rand_sel = rand_id.clone();
        let slider_id = format!("{prefix}-grid-slider");
        let slider_sel = slider_id.clone();
        div()
            .px_1()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(content(0.75))
                            .child("Texture"),
                    )
                    .child(
                        crate::controls::switch(
                            SharedString::from(toggle_id),
                            "Texture",
                            textured,
                            true,
                        )
                        .debug_selector(move || toggle_sel.clone())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.bot.textured = !this.bot.textured;
                            if !this.bot.textured
                                && this.editing_color == Some(EditingColor::Foreground)
                            {
                                this.editing_color = None;
                            }
                            cx.emit(LookChanged {
                                bot: this.bot.clone(),
                            });
                            cx.notify();
                        })),
                    ),
            )
            .when(textured, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .text_size(px(11.))
                                .text_color(content(0.5))
                                .child("Grid Scale")
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1p5()
                                        .child(
                                            div()
                                                .text_color(content(0.8))
                                                .child(format!("{grid}×{grid}")),
                                        )
                                        .child(
                                            div()
                                                .id(SharedString::from(rand_id))
                                                .debug_selector(move || rand_sel.clone())
                                                .role(Role::Button)
                                                .aria_label("Randomize texture seed")
                                                .size(px(22.))
                                                .rounded_md()
                                                .border_1()
                                                .border_color(content(0.14))
                                                .bg(content(0.06))
                                                .hover(|s| s.bg(content(0.14)))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .text_size(px(11.))
                                                .child("🎲")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.randomize_seed(cx);
                                                })),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .id(SharedString::from(slider_id))
                                .debug_selector(move || slider_sel.clone())
                                .role(Role::Slider)
                                .aria_label("Grid scale")
                                .aria_value(format!("{grid}x{grid}"))
                                .relative()
                                .w_full()
                                .h(px(14.))
                                .flex()
                                .items_center()
                                .child(probe.probe())
                                .child(div().w_full().h(px(4.)).rounded_full().bg(content(0.16)))
                                .child(
                                    div()
                                        .absolute()
                                        .top(px(1.))
                                        .left(gpui::relative(fraction * 0.94))
                                        .size(px(12.))
                                        .rounded_full()
                                        .bg(content(1.)),
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                                        this.dragging_grid = true;
                                        this.set_grid_from_x(event.position.x, cx);
                                        cx.stop_propagation();
                                    }),
                                )
                                .on_mouse_move(cx.listener(
                                    |this, event: &gpui::MouseMoveEvent, _, cx| {
                                        if this.dragging_grid
                                            && event.pressed_button == Some(MouseButton::Left)
                                        {
                                            this.set_grid_from_x(event.position.x, cx);
                                        } else {
                                            this.dragging_grid = false;
                                        }
                                    },
                                ))
                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, _| {
                                        this.dragging_grid = false;
                                    }),
                                ),
                        ),
                )
            })
            .into_any_element()
    }
    fn color_buttons_section(&self, prefix: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let textured = self.bot.textured;
        let current_bg = self.bot.color;
        let current_fg = self.bot.resolved_fg_color();
        let bg_active = self.editing_color == Some(EditingColor::Background);
        let fg_active = self.editing_color == Some(EditingColor::Foreground);
        let bg_id = format!("{prefix}-bg-color-button");
        let bg_sel = bg_id.clone();
        let fg_id = format!("{prefix}-fg-color-button");
        let fg_sel = fg_id.clone();
        let swap_id = format!("{prefix}-swap-colors");
        let swap_sel = swap_id.clone();
        div()
            .px_1()
            .flex()
            .flex_col()
            .gap_1p5()
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(content(0.5))
                    .child(if textured { "Colors" } else { "Color" }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .id(SharedString::from(bg_id))
                            .debug_selector(move || bg_sel.clone())
                            .role(Role::Button)
                            .aria_label(if textured {
                                "Background color"
                            } else {
                                "Color"
                            })
                            .aria_expanded(bg_active)
                            .flex_1()
                            .h(px(24.))
                            .px_2()
                            .rounded_md()
                            .border_1()
                            .border_color(if bg_active {
                                accent_color()
                            } else {
                                content(0.15)
                            })
                            .bg(content(if bg_active { 0.12 } else { 0.05 }))
                            .hover(|s| s.bg(content(0.1)))
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap_1p5()
                            .child(avatar::color_swatch_circle(current_bg, None, 10.))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(content(0.85))
                                    .child(if textured { "Background" } else { "Color" }),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_color_picker(EditingColor::Background, cx);
                            })),
                    )
                    .when(textured, |d| {
                        d.child(
                            div()
                                .id(SharedString::from(fg_id))
                                .debug_selector(move || fg_sel.clone())
                                .role(Role::Button)
                                .aria_label("Foreground color")
                                .aria_expanded(fg_active)
                                .flex_1()
                                .h(px(24.))
                                .px_2()
                                .rounded_md()
                                .border_1()
                                .border_color(if fg_active {
                                    accent_color()
                                } else {
                                    content(0.15)
                                })
                                .bg(content(if fg_active { 0.12 } else { 0.05 }))
                                .hover(|s| s.bg(content(0.1)))
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap_1p5()
                                .child(avatar::color_swatch_circle(current_fg, None, 10.))
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(content(0.85))
                                        .child("Foreground"),
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_color_picker(EditingColor::Foreground, cx);
                                })),
                        )
                        .child(
                            div()
                                .id(SharedString::from(swap_id))
                                .debug_selector(move || swap_sel.clone())
                                .role(Role::Button)
                                .aria_label("Swap background and foreground colors")
                                .h(px(24.))
                                .w(px(28.))
                                .flex_shrink_0()
                                .rounded_md()
                                .border_1()
                                .border_color(content(0.15))
                                .bg(content(0.05))
                                .hover(|s| s.bg(content(0.12)))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(11.))
                                .text_color(content(0.85))
                                .child("⇄")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let bg = this.bot.color;
                                    let fg = this.bot.resolved_fg_color();
                                    this.bot.color = fg;
                                    this.bot.fg_color = Some(bg);
                                    if let Some(target) = this.editing_color {
                                        let next_c = match target {
                                            EditingColor::Background => fg,
                                            EditingColor::Foreground => bg,
                                        };
                                        this.custom
                                            .update(cx, |picker, cx| picker.set_color(next_c, cx));
                                    }
                                    cx.emit(LookChanged {
                                        bot: this.bot.clone(),
                                    });
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .into_any_element()
    }
}
impl gpui::Render for LookEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.settings {
            return self.settings_controls(cx);
        }
        const COLOR_NAMES: [&str; 9] = [
            "Blue", "Coral", "Yellow", "Green", "Pink", "Purple", "Teal", "Orange", "Indigo",
        ];
        const FACE_LABELS: [(&str, &str); 5] = [
            ("pills", "3D Pills"),
            ("dots", "Round Dots"),
            ("visor", "Visor"),
            ("carets", "Happy ^ ^"),
            ("wink", "Wink"),
        ];
        let current_face = self.bot.face_style();
        let textured = self.bot.textured;
        let swatch = |id: gpui::ElementId, label: SharedString, ring: bool, dot: AnyElement| {
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(label)
                .aria_selected(ring)
                .flex_1()
                .min_w_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .size(px(22.))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(ring, |d| d.border_2().border_color(content(0.8)))
                        .child(dot),
                )
        };
        div()
            .debug_selector(|| "bot-look-editor".into())
            .w_full()
            .flex()
            .flex_col()
            .gap_2()
            .pb_2()
            .child(
                div()
                    .debug_selector(|| "bot-options-name".into())
                    .px_2p5()
                    .py_1p5()
                    .rounded_lg()
                    .border_1()
                    .border_color(content(0.1))
                    .bg(content(0.05))
                    .text_size(px(13.))
                    .child(self.name.clone()),
            )
            // Face Style Row (5 curved 3D faces)
            .child(
                div()
                    .id("bot-mascot-options")
                    .role(Role::RadioGroup)
                    .aria_label("Face style")
                    .px_1()
                    .flex()
                    .gap(px(2.))
                    .children(
                        FACE_LABELS
                            .iter()
                            .enumerate()
                            .map(|(ix, (face_id, label))| {
                                let selected = current_face == *face_id;
                                let mut preview_bot = self.bot.clone();
                                preview_bot.mascot = (*face_id).into();
                                div()
                                    .id(("bot-mascot", ix))
                                    .debug_selector(move || format!("bot-mascot-{ix}"))
                                    .role(Role::RadioButton)
                                    .aria_label(*label)
                                    .aria_selected(selected)
                                    .flex_1()
                                    .min_w_0()
                                    .h(px(32.))
                                    .rounded_md()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .when(selected, |d| {
                                        d.bg(content(0.1)).border_1().border_color(content(0.2))
                                    })
                                    .when(!selected, |d| d.hover(|s| s.bg(content(0.06))))
                                    .child(avatar::bot_avatar(&preview_bot, "idle", 20., !selected))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.bot.mascot = (*face_id).into();
                                        cx.emit(LookChanged {
                                            bot: this.bot.clone(),
                                        });
                                        cx.notify();
                                    }))
                            }),
                    ),
            )
            // Shape Dropdown
            .child(self.shape_section("bot", cx))
            // Texture Toggle + Seed Die + Grid Scale Slider
            .child(self.texture_and_grid_section("bot", cx))
            // Background & Foreground Color Buttons
            .child(self.color_buttons_section("bot", cx))
            // Preset Color Swatches (9 circular swatches, no custom picker button)
            .child(
                div()
                    .id("bot-color-options")
                    .role(Role::Group)
                    .aria_label("Color")
                    .child(div().px_1().flex().justify_between().gap(px(2.)).children(
                        bot::COLORS_BOT.into_iter().enumerate().map(|(ix, color)| {
                            let pair_fg = TEXTURE_PALETTES
                                .get(ix)
                                .map(|p| p.2)
                                .unwrap_or_else(|| default_foreground_color(color));
                            swatch(
                                ("bot-color", ix).into(),
                                COLOR_NAMES[ix].into(),
                                self.bot.color == color,
                                avatar::color_swatch_circle(
                                    color,
                                    textured.then_some(pair_fg),
                                    16.,
                                ),
                            )
                            .debug_selector(move || format!("bot-color-{ix}"))
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.bot.color = color;
                                    if this.bot.textured {
                                        this.bot.fg_color = Some(pair_fg);
                                    }
                                    this.editing_color = None;
                                    cx.emit(LookChanged {
                                        bot: this.bot.clone(),
                                    });
                                    cx.notify();
                                },
                            ))
                        }),
                    ))
                    .when(self.editing_color.is_some(), |d| {
                        d.child(div().px_1().pt_2().child(self.custom.clone()))
                    }),
            )
            .into_any_element()
    }
}
impl LookEditor {
    fn settings_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        const COLOR_NAMES: [&str; 9] = [
            "Blue", "Coral", "Yellow", "Green", "Pink", "Purple", "Teal", "Orange", "Indigo",
        ];
        const FACE_LABELS: [(&str, &str); 5] = [
            ("pills", "3D Pills"),
            ("dots", "Round Dots"),
            ("visor", "Visor"),
            ("carets", "Happy ^ ^"),
            ("wink", "Wink"),
        ];
        let current_face = self.bot.face_style();
        let textured = self.bot.textured;
        div()
            .id("bot-settings-look-picker")
            .debug_selector(|| "bot-settings-look-picker".into())
            .role(Role::Dialog)
            .aria_label("Mascot and color")
            .track_focus(&self.focus)
            .p_3()
            .w_full()
            .flex()
            .flex_col()
            .gap_2p5()
            .child(
                div()
                    .id("bot-mascot-options")
                    .role(Role::RadioGroup)
                    .aria_label("Face style")
                    .px_1()
                    .flex()
                    .gap_1()
                    .children(
                        FACE_LABELS
                            .iter()
                            .enumerate()
                            .map(|(ix, (face_id, label))| {
                                let selected = current_face == *face_id;
                                let mut preview_bot = self.bot.clone();
                                preview_bot.mascot = (*face_id).into();
                                div()
                                    .id(("settings-mascot", ix))
                                    .role(Role::RadioButton)
                                    .aria_label(*label)
                                    .aria_selected(selected)
                                    .h(px(44.))
                                    .flex_1()
                                    .min_w_0()
                                    .rounded_lg()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .when(selected, |d| {
                                        d.bg(content(0.1)).border_1().border_color(content(0.2))
                                    })
                                    .hover(|d| d.bg(content(0.06)))
                                    .child(avatar::bot_avatar(&preview_bot, "idle", 28., !selected))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.bot.mascot = (*face_id).into();
                                        cx.emit(LookChanged {
                                            bot: this.bot.clone(),
                                        });
                                        cx.notify();
                                    }))
                            }),
                    ),
            )
            .child(self.shape_section("settings-bot", cx))
            .child(self.texture_and_grid_section("settings-bot", cx))
            .child(div().h(px(1.)).bg(content(0.1)))
            .child(self.color_buttons_section("settings-bot", cx))
            .child(
                div()
                    .id("bot-color-options")
                    .role(Role::Group)
                    .aria_label("Color")
                    .px_1()
                    .flex()
                    .justify_between()
                    .gap_1()
                    .children(
                        bot::COLORS_BOT
                            .into_iter()
                            .enumerate()
                            .map(|(index, color)| {
                                let pair_fg = TEXTURE_PALETTES
                                    .get(index)
                                    .map(|p| p.2)
                                    .unwrap_or_else(|| default_foreground_color(color));
                                let selected = self.bot.color == color;
                                div()
                                    .id(("settings-color", index))
                                    .role(Role::Button)
                                    .aria_label(COLOR_NAMES[index])
                                    .aria_selected(selected)
                                    .size(px(26.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .when(selected, |d| d.border_2().border_color(content(0.8)))
                                    .child(avatar::color_swatch_circle(
                                        color,
                                        textured.then_some(pair_fg),
                                        20.,
                                    ))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.bot.color = color;
                                        if this.bot.textured {
                                            this.bot.fg_color = Some(pair_fg);
                                        }
                                        this.editing_color = None;
                                        cx.emit(LookChanged {
                                            bot: this.bot.clone(),
                                        });
                                        cx.notify();
                                    }))
                            }),
                    ),
            )
            .when(self.editing_color.is_some(), |d| {
                d.child(div().px_1().pt_2().child(self.custom.clone()))
            })
            .into_any_element()
    }
}
impl Bots {
    pub(super) fn team_rail(
        &self,
        parent: &Bot,
        liquid: &crate::liquid::Liquid,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let members = self.team_members(&parent.id);
        let retired = members.iter().filter(|m| m.is_retired()).count();
        let expanded = self.retired_teams.contains(&parent.id);
        let rows = members
            .iter()
            .filter(|m| !m.is_retired())
            .map(|m| self.minion_rail_row(parent, m, liquid, cx))
            .collect::<Vec<_>>();
        let history = if expanded {
            members
                .iter()
                .filter(|m| m.is_retired())
                .map(|m| self.minion_rail_row(parent, m, liquid, cx))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let id = parent.id.clone();
        div()
            .debug_selector({
                let id = id.clone();
                move || format!("bot-team-{id}")
            })
            .children(rows)
            .when(retired > 0, |d| {
                d.child(
                    crate::controls::button(
                        SharedString::from(format!("team-history-{id}")),
                        "Retired Minions",
                        true,
                    )
                    .debug_selector({
                        let id = id.clone();
                        move || format!("team-history-{id}")
                    })
                    .flex()
                    .items_center()
                    .aria_expanded(expanded)
                    .ml(px(24.))
                    .h(px(28.))
                    .px_2()
                    .gap_1()
                    .text_size(px(11.))
                    .text_color(content(0.5))
                    .child(icon(
                        if expanded {
                            "chevron-down"
                        } else {
                            "chevron-right"
                        },
                        12.,
                    ))
                    .child(format!("Retired ({retired})"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.retired_teams.remove(&id) {
                            this.retired_teams.insert(id.clone());
                        }
                        cx.notify();
                    })),
                )
            })
            .children(history)
            .into_any_element()
    }
    fn minion_rail_row(
        &self,
        parent: &Bot,
        member: &Bot,
        liquid: &crate::liquid::Liquid,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = member.id.clone();
        let active = self.active.as_ref() == Some(&id);
        let status = self.member_status(&id, cx);
        let meta = member.minion.as_ref().unwrap();
        let details = if meta.specialties.is_empty() {
            meta.role.clone()
        } else {
            format!("{} · {}", meta.role, meta.specialties.join(", "))
        };
        let state = if member.is_retired() {
            "Retired"
        } else {
            match status {
                "working" => "Working",
                "needs-you" => "Needs attention",
                _ => "Idle",
            }
        };
        crate::controls::button(
            SharedString::from(format!("minion-rail-{id}")),
            SharedString::from(format!(
                "{}, Minion of {}, {details}, {state}",
                member.label(),
                parent.label()
            )),
            true,
        )
        .debug_selector({
            let id = id.clone();
            move || format!("minion-rail-{id}")
        })
        .flex()
        .items_center()
        .aria_selected(active)
        .h(px(40.))
        .pl(px(24.))
        .pr_2()
        .gap_2()
        .relative()
        .child(liquid.row(format!("bot:{id}"), active))
        .child(avatar::minion_avatar(parent, &id, status, 16., false))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .truncate()
                        .text_size(px(13.))
                        .text_color(content(1.))
                        .child(member.label()),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(11.))
                        .text_color(content(0.5))
                        .child(details),
                ),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(content(0.5))
                .child(state),
        )
        .on_click(cx.listener(move |this, _, window, cx| this.open(&id, window, cx)))
        .into_any_element()
    }

    fn open_settings_look(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bot) = self.settings_bot(id).cloned() else {
            return;
        };
        let editor = cx.new(|cx| {
            let mut editor = LookEditor::new(bot, cx);
            editor.settings = true;
            editor
        });
        let focus = editor.read(cx).focus.clone();
        let changed = cx.subscribe(&editor, |this, _, event: &LookChanged, cx| {
            this.edit_settings_bot(
                &event.bot.id,
                |bot| {
                    bot.mascot = event.bot.mascot.clone();
                    bot.shape = event.bot.shape.clone();
                    bot.color = event.bot.color;
                    bot.fg_color = event.bot.fg_color;
                    bot.textured = event.bot.textured;
                    bot.seed = event.bot.seed;
                    bot.grid = event.bot.grid;
                },
                cx,
            );
        });
        let menu = cx.new(|cx| {
            Menu::new(
                "Mascot and color",
                vec![MenuItem::Custom(editor.into(), focus)],
                window,
                cx,
            )
            .width(280.)
        });
        let closed = cx.subscribe(&menu, |this, _, _: &MenuEvent, cx| {
            this.picker = None;
            cx.notify();
        });
        let anchor = Anchor::Slot(self.settings_anchor.clone());
        self.picker = Some(OpenPicker {
            menu,
            anchor,
            width: 280.,
            align: crate::popover::Align::Center,
            _events: vec![changed, closed],
        });
        cx.notify();
    }
    pub(super) fn field(
        &mut self,
        key: &str,
        label: &str,
        text: String,
        multi: bool,
        cx: &mut Context<Self>,
    ) -> Entity<Input> {
        if let Some(field) = self.fields.get(key) {
            return field.clone();
        }
        let label = label.to_string();
        let field = cx.new(|cx| {
            let mut f = if key.starts_with("channel-draft:") {
                Input::channel_message(cx)
            } else if multi {
                Input::markdown_editor(&label, cx)
            } else if key == "memory-fact" || key.starts_with("memory-edit:") {
                Input::memory_fact(&label, cx)
            } else {
                Input::field(label.clone(), label.clone(), content(0.35), cx)
            };
            f.set_font_size(if key == "memory-fact" || key.starts_with("memory-edit:") {
                12.
            } else {
                13.
            });
            f.set_placeholder(label, cx);
            if key == "bot-name" {
                f.set_max_chars(40);
            }
            f.set_text(text, cx);
            f
        });
        let key_owned = key.to_string();
        self.observations.push(
            cx.subscribe(&field, move |this, input, _: &input::Changed, cx| {
                if this
                    .fields
                    .get(&key_owned)
                    .is_none_or(|current| current.entity_id() != input.entity_id())
                {
                    return;
                }
                if key_owned.starts_with("channel-draft:")
                    && let Err(error) = locked(&this.store)
                        .and_then(|db| db.save_setting(&key_owned, &input.read(cx).content))
                {
                    this.error = Some(error);
                }
                if key_owned == "bot-name" {
                    let name = input.read(cx).content.trim().chars().take(40).collect();
                    if let Some(id) = this
                        .creation
                        .as_ref()
                        .map(|draft| draft.bot.id.clone())
                        .or(this.active.clone())
                    {
                        this.edit_settings_bot(&id, |bot| bot.name = name, cx);
                    }
                }
                cx.notify();
            }),
        );
        self.fields.insert(key.into(), field.clone());
        if let Some(channel) = key.strip_prefix("channel-draft:") {
            let channel = channel.to_owned();
            self.observations.push(cx.subscribe(
                &field,
                move |this, _, _: &input::Submitted, cx| {
                    if this.channels.open && this.channels.selected.as_deref() == Some(&channel) {
                        this.send_channel_message(&channel, cx);
                    }
                },
            ));
        }
        field
    }
    pub(super) fn value(&self, key: &str, cx: &App) -> String {
        self.fields
            .get(key)
            .map(|f| f.read(cx).content.to_string())
            .unwrap_or_default()
    }
    pub(crate) fn finish_reorder(&mut self, cx: &mut Context<Self>) {
        if let Some((id, _, Some(over))) = self.drag.take()
            && self.skip_click
        {
            let mut ids = self.rail_ids.clone();
            if let Some((from, to)) = ids
                .iter()
                .position(|s| s == &id)
                .zip(ids.iter().position(|s| s == &over))
            {
                let id = ids.remove(from);
                ids.insert(to, id);
                self.roster.reorder(&ids);
                self.save(cx);
            }
        }
    }
}

#[cfg(test)]
mod minion_ui_tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};
    use std::sync::{Arc, Mutex};
    use vorkspace_core::{bot::MinionMetadata, store::Store};

    #[gpui::test]
    fn minions_nest_under_parent_and_retired_history_stays_read_only_when_disabled(
        cx: &mut TestAppContext,
    ) {
        vorkspace_ui::clock::use_test_clock(cx);
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(
            Store::open(&directory.path().join("ui.sqlite")).unwrap(),
        ));
        let cwd = directory.path().to_string_lossy().to_string();
        let (active, retired) = {
            let db = locked(&store).unwrap();
            (
                db.create("codex", &cwd).unwrap(),
                db.create("codex", &cwd).unwrap(),
            )
        };
        let window = cx
            .update(|cx| {
                cx.open_window(Default::default(), |_, cx| {
                    cx.new(|cx| {
                        Shell::new(
                            store,
                            vec![active.clone(), retired.clone()],
                            cwd.clone(),
                            None,
                            cx,
                        )
                    })
                })
            })
            .unwrap();
        cx.run_until_parked();
        let manager = window
            .update(cx, |shell, _, _| shell.bot_manager())
            .unwrap();
        manager.update(cx, |bots, cx| {
            let parent = Bot {
                id: "team-parent".into(),
                name: "Parent".into(),
                projects: vec![cwd.clone()],
                ..Default::default()
            };
            bots.roster.bots.push(parent);
            for (id, summary, retired) in [
                ("team-active", &active, false),
                ("team-retired", &retired, true),
            ] {
                bots.roster.bots.push(Bot {
                    id: id.into(),
                    name: id.into(),
                    session_id: Some(summary.id),
                    minion: Some(MinionMetadata {
                        parent_id: "team-parent".into(),
                        role: "Reviewer".into(),
                        specialties: vec!["Storage".into()],
                        retired,
                        ..Default::default()
                    }),
                    ..Default::default()
                });
            }
            let mut prefs = bots.prefs.clone();
            prefs.minions = false;
            bots.set_preferences(prefs, cx);
            bots.sync_team_histories(cx);
            assert!(bots.session_user_read_only(active.id));
            assert!(bots.session_user_read_only(retired.id));
            assert!(bots.hidden_session(active.id) && bots.hidden_session(retired.id));
            let _ = bots.rail(None, &crate::liquid::Liquid::default(), cx);
            assert_eq!(bots.rail_ids, vec!["team-parent"]);
        });
        window
            .update(cx, |shell, window, cx| {
                // Materialize a legacy project pane before selecting the Bot row.
                let legacy = shell.bot_ensure_session(retired.id, cx).unwrap();
                assert!(legacy.read(cx).user_read_only());
                manager.update(cx, |bots, cx| bots.open("team-retired", window, cx));
            })
            .unwrap();
        cx.run_until_parked();
        manager.read_with(cx, |bots, cx| {
            let chat = bots.chat().unwrap();
            let legacy = bots
                .owner
                .upgrade()
                .unwrap()
                .read(cx)
                .bot_session_entity(retired.id)
                .unwrap();
            assert_eq!(
                chat.entity_id(),
                legacy.entity_id(),
                "history and restored pane share one view"
            );
            assert!(chat.read(cx).user_read_only());
            assert!(bots.retired_teams.contains("team-parent"));
            assert!(bots.settings.is_none());
        });
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("team-read-only-history").is_some());
        assert!(visual.debug_bounds("team-history-attribution").is_some());
    }
}
