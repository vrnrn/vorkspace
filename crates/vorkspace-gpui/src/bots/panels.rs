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
pub(super) enum OverviewTab {
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
pub(super) enum Route {
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
    pub(super) fn select_overview(&mut self, tab: OverviewTab, cx: &mut Context<Self>) {
        if tab != OverviewTab::Changes {
            self.pause_changes(cx);
        }
        if self.route.is_some() {
            self.save_open_file(cx);
            self.route = None;
            cx.notify();
        }
        if self.overview_tab != tab {
            self.overview_reveal =
                Some(crate::project_tabs::TabReveal::new(tab, self.overview_tab));
            self.overview_tab = tab;
            cx.notify();
        }
    }
    pub(crate) fn overview_controls(
        &self,
        expand_label: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(bot) = self.current() else {
            return div().into_any_element();
        };
        let progress = self
            .overview_reveal
            .as_ref()
            .and_then(|r| r.progress(self.overview_tab, window));
        let tabs = [
            (
                OverviewTab::Sessions,
                "Sessions",
                "message-multiple",
                bot.launches.len(),
            ),
            (
                OverviewTab::Documents,
                "Documents",
                "file",
                bot.documents.len(),
            ),
            (OverviewTab::Habits, "Habits", "clock", self.habits.len()),
            (
                OverviewTab::Changes,
                "Changes",
                "file-diff",
                self.project_changes.count(&bot.id),
            ),
            (
                OverviewTab::Todos,
                "To-dos",
                "check-circle",
                self.open_todos(&bot.id, cx),
            ),
        ];
        div()
            .id("bot-overview-tabs")
            .debug_selector(|| "bot-overview-tabs".into())
            .role(Role::TabList)
            .aria_label("Bot details")
            .h(px(TITLE_BAR_HEIGHT))
            .flex_1()
            .min_w_0()
            .mr(px(2.))
            .flex()
            .items_center()
            .gap(px(2.))
            .children(tabs.into_iter().map(|(tab, label, glyph, count)| {
                let selected = self.overview_tab == tab;
                let unfolded = match progress {
                    Some((_, p)) if selected => p,
                    Some((closing, p)) if tab == closing => 1. - p,
                    _ if selected => 1.,
                    _ => 0.,
                };
                let id = match tab {
                    OverviewTab::Sessions => "bot-overview-Sessions",
                    OverviewTab::Documents => "bot-overview-Documents",
                    OverviewTab::Habits => "bot-overview-Habits",
                    OverviewTab::Changes => "bot-overview-Changes",
                    OverviewTab::Todos => "bot-overview-Todos",
                };
                crate::project_tabs::tab_button(
                    id,
                    (label, count),
                    glyph,
                    selected,
                    tab == OverviewTab::Changes && count > 0 && !selected,
                    if expand_label { unfolded } else { 0. },
                    window,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.select_overview(tab, cx)))
            }))
            .child(crate::shell::titlebar_drag_area().flex_1().min_w_0())
            .when(self.overview_tab == OverviewTab::Habits, |d| {
                d.child(
                    icon_button("bot-new-habit", "New habit", "plus")
                        .debug_selector(|| "bot-new-habit".into())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.route = Some(Route::NewHabit);
                            this.clear_habit_fields();
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }
    pub(super) fn overview_panel(
        &mut self,
        bot: &Bot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_changes_summary(bot, cx);
        if self.overview_tab == OverviewTab::Changes {
            self.ensure_changes(bot, window, cx);
        } else {
            self.pause_changes(cx);
        }
        let body = match self.overview_tab {
            OverviewTab::Sessions => self.session_list(bot, cx),
            OverviewTab::Habits => self.habits_list(bot, cx),
            OverviewTab::Documents => self.document_list(bot, cx),
            OverviewTab::Changes => self.changes_list(bot, cx),
            OverviewTab::Todos => self.todo_list(bot, cx),
        };
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .child(body)
            .into_any_element()
    }
    /// Open items on the Bot's board: its open board's, which shows changes
    /// at once, or else the stored count.
    pub(super) fn open_todos(&self, id: &str, cx: &App) -> usize {
        let key = vorkspace_core::todos::TodoBoard::Bot(id.into());
        self.todo_board
            .as_ref()
            .map(|board| board.read(cx))
            .filter(|board| board.board() == Some(&key))
            .and_then(|board| board.open_count())
            .or_else(|| self.todo_counts.get(&key.key()).copied())
            .unwrap_or(0)
    }
    pub(super) fn load_todo_counts(&mut self, cx: &mut Context<Self>) {
        self.todo_counts_generation += 1;
        let generation = self.todo_counts_generation;
        let store = self.store.clone();
        let work = cx
            .background_executor()
            .spawn(async move { locked(&store)?.open_todo_counts() });
        cx.spawn(async move |this, cx| {
            let counts = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.todo_counts_generation != generation {
                    return;
                }
                if let Ok(counts) = counts {
                    this.todo_counts = counts;
                    cx.notify();
                }
            });
        })
        .detach();
    }
    fn todo_list(&mut self, bot: &Bot, cx: &mut Context<Self>) -> AnyElement {
        let store = self.store.clone();
        let board = self
            .todo_board
            .get_or_insert_with(|| cx.new(|cx| crate::todos::TodoBoardView::new(store, false, cx)))
            .clone();
        let key = vorkspace_core::todos::TodoBoard::Bot(bot.id.clone());
        board.update(cx, |board, cx| board.set_board(Some(key), cx));
        div().size_full().child(board).into_any_element()
    }
    fn session_list(&mut self, bot: &Bot, cx: &mut Context<Self>) -> AnyElement {
        let filter = self.field(
            "bot-session-search",
            "Search sessions…",
            String::new(),
            false,
            cx,
        );
        let query = filter.read(cx).content.to_lowercase();
        let views = self
            .owner
            .upgrade()
            .map(|owner| owner.read(cx).bot_session_views(cx))
            .unwrap_or_default();
        let launches: Vec<_> = bot
            .launches
            .iter()
            .rev()
            .filter(|l| {
                self.session_filters
                    .matches(l, views.iter().find(|v| v.id == l.session_id))
                    && format!(
                        "{} {} {}",
                        views
                            .iter()
                            .find(|v| v.id == l.session_id)
                            .map(|v| v.title.as_str())
                            .unwrap_or(&l.title),
                        views
                            .iter()
                            .find(|v| v.id == l.session_id)
                            .map(|v| v.cwd.as_str())
                            .unwrap_or(&l.cwd),
                        l.harness
                    )
                    .to_lowercase()
                    .contains(&query)
            })
            .cloned()
            .collect();
        let search_bar = div()
            .h(px(36.))
            .px_2()
            .border_b_1()
            .border_color(crate::right_panel::stroke())
            .flex()
            .items_center()
            .gap_2()
            .child(icon("search", 12.))
            .child(div().flex_1().min_w_0().child(filter))
            .child(
                icon_button("bot-session-filter", "Filter Bot sessions", "filter")
                    .relative()
                    .child(self.session_filter_anchor.probe())
                    .on_click(cx.listener(|this, _, window, cx| this.filter_menu(window, cx))),
            );
        let no_matches = launches.is_empty() && !bot.launches.is_empty();
        div().size_full().flex().flex_col()
            .when(!bot.launches.is_empty(),|d|d.child(search_bar))
            .child(div().id("bot-session-list").debug_selector(|| "bot-session-list".into()).flex_1().min_h_0().overflow_y_scroll().px(px(6.)).py_2().flex().flex_col().gap_1()
                .when(bot.launches.is_empty(),|d|d.child(empty_state("message-multiple",&format!("Ask {} to start a session on one of your projects, like a bug fix or a review, and it shows up here.",bot.label()))))
                .when(no_matches,|d|d.child(empty_state("search-01",if query.trim().is_empty(){"No sessions match these filters."}else{"No sessions match this search."})))
                .children(launches.into_iter().map(|launch|{
                    // Sidebar.tsx's SessionCard with `modelInFooter`.
                    let id=launch.session_id;let state=views.iter().find(|v|v.id==id);
                    let title=state.map(|s|s.title.clone()).unwrap_or(launch.title.clone());
                    let cwd=state.map(|s|&s.cwd).unwrap_or(&launch.cwd);
                    let project=bot.projects.iter().find(|p|vorkspace_core::files::path_key(p)==vorkspace_core::files::path_key(cwd)).unwrap_or(cwd);
                    let project=std::path::Path::new(project).file_name().map_or(project.clone(),|n|n.to_string_lossy().to_string());
                    let branch=state.and_then(|s|s.branch.clone());
                    let model:SharedString=state.map(|s|s.model.clone()).filter(|model| !model.is_empty()).unwrap_or_else(||launch.model.clone().unwrap_or_default().into());
                    let status=match state{
                        Some(s) if s.needs_input=>div().flex().items_center().gap_1().text_color(rgb(0xfbbf24)).child(icon("circle-alert",12.).text_color(rgb(0xfbbf24))).child("Need approval").into_any_element(),
                        Some(s) if s.busy=>div().text_color(vorkspace_ui::theme::accent_color()).child("Working...").into_any_element(),
                        Some(s)=>div().text_color(content(0.45)).child(vorkspace_core::time::relative(s.updated_at,vorkspace_core::time::now_ms())).into_any_element(),
                        None=>div().text_color(content(0.45)).child("Unavailable").into_any_element(),
                    };
                    let place=match &branch{Some(b)=>format!("{project} · {b}"),None=>project};
                    div().id(("bot-launch",id as usize)).debug_selector(move||format!("bot-launch-{id}")).group("bot-launch").role(Role::Button).aria_label(title.clone())
                        .px(px(10.)).py_2().rounded_md().border_1().border_color(if state.is_some_and(|s|s.needs_input){content(0.3)}else{gpui::transparent_black()})
                        .when(state.is_some_and(|s|s.needs_input),|d|d.bg(content(0.2)))
                        .hover(|s|s.bg(content(0.05)))
                        .when(state.is_none(),|d|d.opacity(0.5))
                        .flex().flex_col()
                        .child(div().flex().min_w_0().items_center().gap_1p5()
                            .child(div().min_w_0().truncate().text_size(px(13.)).line_height(px(18.)).font_weight(FontWeight::SEMIBOLD).text_color(content(1.)).child(title))
                            .child(div().ml_auto().pl_2().flex_shrink_0().text_size(px(11.)).child(status)))
                        .child(div().mt_1().flex().min_w_0().items_center().gap_2()
                            .child(div().flex_1().min_w_0().flex().items_center().gap_1().text_size(px(11.)).text_color(content(0.45))
                                .child(icon(if branch.is_some(){"git-branch"}else{"folder-01"},12.).text_color(content(0.45)))
                                .child(div().min_w_0().truncate().child(place)))
                            .child(div().flex_shrink_0().flex().items_center().text_size(px(11.)).text_color(content(0.5))
                                .child(div().max_w(px(0.)).overflow_hidden().whitespace_nowrap().group_hover("bot-launch",|s|s.max_w(px(128.)).pr(px(6.))).child(model))
                                .child(provider_icon(&launch.harness,14.))))
                        .on_click(cx.listener(move|this,_,_,cx|{this.route=Some(Route::Session(id));cx.notify();}))
}))).into_any_element()
    }
    fn document_list(&self, bot: &Bot, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("bot-document-list")
            .size_full()
            .overflow_y_scroll()
            .p_2()
            .flex()
            .flex_col()
            .gap_px()
            .when(bot.documents.is_empty(), |d| {
                d.child(empty_state(
                    "file",
                    &format!(
                        "Ask {} to write something up, like a plan or a summary, and it shows up here.",
                        bot.label()
                    ),
                ))
            })
            .children(bot.documents.iter().rev().map(|document| {
                // BotOverview's DocumentRow: laid out like a habit.
                let id = document.id.clone();
                let label = if document.kind == "html" {
                    "Visual"
                } else {
                    "Document"
                };
                div()
                    .id(SharedString::from(format!("bot-document-{id}")))
                    .debug_selector({
                        let id = id.clone();
                        move || format!("bot-document-{id}")
                    })
                    .role(Role::Button)
                    .aria_label(format!("Open {} {}", label.to_lowercase(), document.title))
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2p5()
                    .rounded_md()
                    .px_2()
                    .py_2()
                    .hover(|s| s.bg(content(0.05)))
                    .child(
                        div()
                            .size(px(28.))
                            .flex_shrink_0()
                            .rounded_md()
                            .bg(content(0.06))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                icon(
                                    if document.kind == "html" {
                                        "chart-square"
                                    } else {
                                        "file"
                                    },
                                    14.,
                                )
                                .text_color(content(0.6)),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.))
                                    .line_height(px(20.))
                                    .text_color(content(0.85))
                                    .child(document.title.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(11.))
                                    .line_height(px(16.))
                                    .text_color(content(0.4))
                                    .child(if document.summary.trim().is_empty() {
                                        label.to_string()
                                    } else {
                                        document.summary.clone()
                                    }),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_document(&id, cx);
                        this.route = Some(Route::Document(id.clone()));
                        cx.notify();
                    }))
            }))
            .into_any_element()
    }
    pub(super) fn page_header(&self, title: &str, cx: &mut Context<Self>) -> Div {
        div()
            .h(px(40.))
            .flex_shrink_0()
            .min_w_0()
            .overflow_hidden()
            .border_b_1()
            .border_color(content(0.1))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .child(
                icon_button("bot-page-back", "Back", "chevron-left")
                    .debug_selector(|| "bot-page-back".into())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.route = None;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(title.to_string()),
            )
    }
    pub(super) fn route_page(
        &mut self,
        bot: &Bot,
        route: Route,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let interactive = self.route.as_ref() == Some(&route);
        match route {
            Route::Review => div()
                .size_full()
                .flex()
                .flex_col()
                .child(self.page_header("Changes", cx))
                .when_some(self.review.clone(), |d, review| {
                    d.child(div().flex_1().min_h_0().child(review))
                })
                .into_any_element(),
            Route::Session(id) => {
                let view = self.owner.upgrade().and_then(|owner| {
                    owner.update(cx, |owner, cx| owner.bot_ensure_session(id, cx))
                });
                if let Some(view) = &view {
                    view.update(cx, |v, cx| v.load(cx));
                }
                let title = view
                    .as_ref()
                    .map(|v| v.read(cx).summary.title.clone())
                    .unwrap_or_else(|| "Session unavailable".into());
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(
                        self.page_header(&title, cx).child(
                            icon_button(
                                "bot-open-session",
                                "Open session in workspace",
                                "external-link",
                            )
                            .on_click(
                                cx.listener(move |this, _, w, cx| this.open_workspace(id, w, cx)),
                            ),
                        ),
                    )
                    .when_some(view, |d, view| {
                        d.child(div().flex_1().min_h_0().child(view))
                    })
                    .into_any_element()
            }
            Route::Document(id) => {
                let Some(document) = self
                    .work
                    .documents
                    .get(&id)
                    .or_else(|| {
                        bot.documents
                            .iter()
                            .find(|d| d.id == id && !bot.desktop_document_ids.contains(&id))
                    })
                    .cloned()
                else {
                    return empty(if self.work.loading_documents.contains(&id) {
                        "Loading document…"
                    } else {
                        "Document unavailable"
                    });
                };
                let view: AnyElement = if document.kind == "html"
                    && (self.settings.is_some()
                        || self.menu.is_some()
                        || self.picker.is_some()
                        || self.background.is_some())
                {
                    div().into_any_element()
                } else if document.kind == "html" {
                    let html = self
                        .html_documents
                        .entry(id.clone())
                        .or_insert_with(|| {
                            cx.new(|_| super::html::Html::new(document.body.clone()))
                        })
                        .clone();
                    html.update(cx, |html, _| html.set_interactive(interactive));
                    html.into_any_element()
                } else {
                    self.documents
                        .entry(id.clone())
                        .or_insert_with(|| {
                            cx.new(|cx| {
                                crate::transcript::TranscriptView::document(
                                    document.body.clone(),
                                    cx,
                                )
                            })
                        })
                        .clone()
                        .into_any_element()
                };
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(
                        self.page_header(&document.title, cx).child(
                            icon_button("bot-document-copy", "Copy document", "response-copy")
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                        document.body.clone(),
                                    ))
                                }),
                        ),
                    )
                    .child(div().flex_1().min_h_0().child(view))
                    .into_any_element()
            }
            Route::Habit(id) => self.habit_page(bot, Some(&id), window, cx),
            Route::NewHabit => self.habit_page(bot, None, window, cx),
        }
    }
    pub(super) fn settings_modal(
        &mut self,
        bot: &Bot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if bot.is_minion() {
            return None;
        }

        if self
            .creation
            .as_ref()
            .is_some_and(|draft| draft.bot.id != bot.id)
        {
            return None;
        }
        let creating = self
            .creation
            .as_ref()
            .is_some_and(|draft| draft.bot.id == bot.id);
        let section = if creating {
            self.settings.unwrap_or(SettingsSection::General)
        } else {
            self.settings?
        };
        let size = window.viewport_size();
        let width = 880f32.min(size.width.as_f32() - 24.);
        let navigation_width = if width < 660. { 140. } else { 192. };
        let height = 680f32.min(size.height.as_f32() - 32.);
        let delete_anchor = AnchorSlot::default();
        let body = match section {
            SettingsSection::General => {
                self.general_settings(bot, delete_anchor.clone(), window, cx)
            }
            SettingsSection::Soul => self.soul_page(bot, cx),
            SettingsSection::Memory => self.memory_page(bot, window, cx),
        };
        self.bind_settings_blurs(window, cx);
        let weak = cx.weak_entity();
        Some(
            div()
                .absolute()
                .size_full()
                .child(
                    gpui::deferred(
                        gpui::anchored()
                            .position(gpui::point(px(0.), px(0.)))
                            .child(
                                div()
                                    .w(size.width)
                                    .h(size.height)
                                    .bg(gpui::black().opacity(0.4))
                                    .occlude(),
                            ),
                    )
                    .with_priority(69),
                )
                .child(
                    popover(
                        "bot-settings-modal",
                        Anchor::Point(gpui::point(
                            px((size.width.as_f32() - width) / 2.),
                            px((size.height.as_f32() - height) / 2.),
                        )),
                    )
                    .ignore(self.notice_region.clone())
                    .when_some(self.picker.as_ref(), |popover, picker| {
                        // Pickers are deferred above the dialog and can extend past
                        // its bounds. Their surfaces still belong to this setup.
                        let menu = picker.menu.read(cx);
                        popover
                            .ignore(menu.surface_region())
                            .ignore(menu.submenu_region())
                    })
                    .modal_surface(true)
                    .without_animation()
                    .priority(70)
                    .width(width)
                    .height(height)
                    .gap(0.)
                    .on_dismiss(move |window, cx| {
                        let _ = weak.update(cx, |this, cx| {
                            if creating {
                                this.cancel_creation(window, cx);
                                return;
                            }
                            this.save_open_file(cx);
                            this.settings = None;
                            this.picker = None;
                            cx.notify();
                        });
                    })
                    .child(
                        div()
                            .id("bot-settings-dialog")
                            .debug_selector(|| "bot-settings-dialog".into())
                            .role(Role::Dialog)
                            .track_focus(&self.settings_focus)
                            .aria_label(if creating {
                                "New Bot".into()
                            } else {
                                format!("{} settings", bot.label())
                            })
                            .h_full()
                            .w_full()
                            .flex()
                            .relative()
                            .child(
                                div()
                                    .id("bot-settings-navigation")
                                    .debug_selector(|| "bot-settings-navigation".into())
                                    .role(Role::Navigation)
                                    .aria_label("Settings sections")
                                    .w(px(navigation_width))
                                    .flex_shrink_0()
                                    .h_full()
                                    .border_r_1()
                                    .border_color(content(0.04))
                                    .p_2()
                                    .flex()
                                    .flex_col()
                                    .gap_px()
                                    .child(
                                        div()
                                            .px_2p5()
                                            .pt_1p5()
                                            .pb_2()
                                            .text_size(px(11.))
                                            .text_color(content(0.45))
                                            .child(if creating { "New Bot" } else { "Settings" }),
                                    )
                                    .children(
                                        [
                                            (SettingsSection::General, "General", "settings"),
                                            (SettingsSection::Soul, "Soul", "sparkles"),
                                            (SettingsSection::Memory, "Memory", "sticky-note"),
                                        ]
                                        .into_iter()
                                        .filter(|(tab, _, _)| {
                                            !creating || *tab != SettingsSection::Memory
                                        })
                                        .map(
                                            |(tab, label, glyph)| {
                                                let row_id = format!("bot-settings-{label}");
                                                div()
                                                    .id(SharedString::from(row_id.clone()))
                                                    .debug_selector(move || row_id.clone())
                                                    .role(Role::Button)
                                                    .aria_label(label)
                                                    .w_full()
                                                    .h(px(32.))
                                                    .flex_shrink_0()
                                                    .flex()
                                                    .flex_row()
                                                    .items_center()
                                                    .justify_start()
                                                    .text_align(gpui::TextAlign::Left)
                                                    .gap(px(10.))
                                                    .px(px(10.))
                                                    .py(px(6.))
                                                    .rounded_lg()
                                                    .text_size(px(13.))
                                                    .line_height(px(20.))
                                                    .text_color(content(if section == tab {
                                                        1.
                                                    } else {
                                                        0.65
                                                    }))
                                                    .when(section == tab, |d| d.bg(content(0.08)))
                                                    .when(section != tab, |d| {
                                                        d.hover(|d| {
                                                            d.bg(content(0.05))
                                                                .text_color(content(1.))
                                                        })
                                                    })
                                                    .child(
                                                        div()
                                                            .id(SharedString::from(format!(
                                                                "bot-settings-icon-{label}"
                                                            )))
                                                            .debug_selector(move || {
                                                                format!("bot-settings-icon-{label}")
                                                            })
                                                            .size(px(14.))
                                                            .flex_shrink_0()
                                                            .child(icon(glyph, 14.).text_color(
                                                                content(if section == tab {
                                                                    1.
                                                                } else {
                                                                    0.65
                                                                }),
                                                            )),
                                                    )
                                                    .child(
                                                        div()
                                                            .id(SharedString::from(format!(
                                                                "bot-settings-label-{label}"
                                                            )))
                                                            .debug_selector(move || {
                                                                format!(
                                                                    "bot-settings-label-{label}"
                                                                )
                                                            })
                                                            .child(label),
                                                    )
                                                    .on_click(cx.listener(
                                                        move |this, _, window, cx| {
                                                            this.save_open_file(cx);
                                                            this.settings = Some(tab);
                                                            if tab == SettingsSection::Memory
                                                                && let Some(id) =
                                                                    this.active.clone()
                                                            {
                                                                this.reload_files(&id, cx);
                                                            }
                                                            window.focus(&this.settings_focus, cx);
                                                            cx.notify();
                                                        },
                                                    ))
                                            },
                                        ),
                                    )
                                    .when(!creating, |d| {
                                        let id = bot.id.clone();
                                        d.child(div().flex_1()).child(
                                            div()
                                                .id("bot-delete")
                                                .debug_selector(|| "bot-delete".into())
                                                .role(Role::Button)
                                                .aria_label("Delete bot")
                                                .relative()
                                                .w_full()
                                                .h(px(32.))
                                                .flex_shrink_0()
                                                .flex()
                                                .items_center()
                                                .gap(px(10.))
                                                .px(px(10.))
                                                .rounded_lg()
                                                .text_size(px(13.))
                                                .line_height(px(20.))
                                                .text_color(rgb(0xff6467))
                                                .hover(|d| d.bg(rgb(0xff6467).opacity(0.08)))
                                                .child(delete_anchor.probe())
                                                .child(icon("trash", 14.).text_color(rgb(0xff6467)))
                                                .child("Delete bot…")
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.save_open_file(cx);
                                                        this.delete_bot(&id, window, cx);
                                                    },
                                                )),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .flex()
                                    .flex_col()
                                    .child(body)
                                    .when(creating || section == SettingsSection::General, |d| {
                                        d.child(self.settings_footer(bot, creating, cx))
                                    }),
                            )
                            .child(
                                div().absolute().right_3().top_1p5().child(
                                    icon_button(
                                        "bot-settings-close",
                                        "Close Bot settings",
                                        "close",
                                    )
                                    .size(px(28.))
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            if creating {
                                                this.cancel_creation(window, cx);
                                                return;
                                            }
                                            this.save_open_file(cx);
                                            this.settings = None;
                                            cx.notify();
                                        },
                                    )),
                                ),
                            ),
                    ),
                )
                .into_any_element(),
        )
    }
    fn settings_footer(&self, bot: &Bot, creating: bool, cx: &mut Context<Self>) -> AnyElement {
        let enabled = !bot.name.trim().is_empty();
        let reset = bot.id.clone();
        div()
            .id("bot-settings-footer")
            .debug_selector(|| "bot-settings-footer".into())
            .flex_shrink_0()
            .min_h(px(56.))
            .px_6()
            .py_3()
            .border_t_1()
            .border_color(content(0.08))
            .flex()
            .items_center()
            .justify_end()
            .gap_2()
            .when(creating, |d| {
                d.child(
                    button("bot-create-cancel", "Cancel").on_click(
                        cx.listener(|this, _, window, cx| this.cancel_creation(window, cx)),
                    ),
                )
                .child(
                    div()
                        .id("bot-create")
                        .debug_selector(|| "bot-create".into())
                        .role(Role::Button)
                        .aria_label("Create")
                        .aria_description(if enabled {
                            "Create this named Bot"
                        } else {
                            "Enter a Bot name to enable Create"
                        })
                        .h(px(30.))
                        .min_w(px(76.))
                        .px_4()
                        .rounded_md()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .bg(rgb(0x0a84ff).opacity(if enabled { 1. } else { 0.28 }))
                        .text_color(if enabled { black(1.) } else { content(0.45) })
                        .when(enabled, |d| d.hover(|d| d.bg(rgb(0x409cff))))
                        .child("Create")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if enabled {
                                this.confirm_creation(window, cx);
                            }
                        })),
                )
            })
            .when(!creating, |d| {
                d.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.))
                        .text_color(content(0.55))
                        .child("Reset messages; keep soul, memory and habits."),
                )
                .child(
                    button("bot-reset", "Reset")
                        .debug_selector(|| "bot-reset".into())
                        .text_color(rgb(0xff6467))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.reset_conversation(&reset, window, cx)
                        })),
                )
            })
            .into_any_element()
    }
    fn general_settings(
        &mut self,
        bot: &Bot,
        delete_anchor: AnchorSlot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let creating = self
            .creation
            .as_ref()
            .is_some_and(|draft| draft.bot.id == bot.id);
        let narrow = window.viewport_size().width < px(780.);
        let name = self.field("bot-name", "Bot name", bot.name.clone(), false, cx);
        name.update(cx, |f, cx| {
            f.set_font_size(15.);
            f.set_line_height(20.);
            f.set_centered(true);
            f.set_placeholder(
                if creating {
                    "Name your bot".into()
                } else {
                    bot.label()
                },
                cx,
            );
        });
        let look = bot.id.clone();
        let add = bot.id.clone();
        let chat = self.chat();
        let (model, provider, mode, model_options) = if let Some(draft) = &self.creation {
            let model = draft
                .model()
                .map(|model| {
                    crate::pickers::model_display_name(draft.provider, &model.id, &model.name)
                })
                .unwrap_or_else(|| {
                    crate::pickers::model_name(draft.provider, draft.settings.model.as_deref())
                });
            (
                model,
                draft.provider.id().to_owned(),
                draft.settings.access,
                crate::conversation::bot_settings::model_options(
                    draft.provider,
                    draft.model(),
                    &draft.settings,
                ),
            )
        } else {
            (
                chat.as_ref()
                    .map(|view| view.read(cx).bot_model_label())
                    .unwrap_or_else(|| "Loading…".into()),
                chat.as_ref()
                    .map(|view| view.read(cx).summary.provider.clone())
                    .unwrap_or_else(|| "codex".into()),
                chat.as_ref()
                    .map(|view| view.read(cx).settings.access)
                    .unwrap_or(AccessMode::Auto),
                chat.as_ref()
                    .map(|view| view.read(cx).bot_model_options())
                    .unwrap_or_default(),
            )
        };
        let access_icon = match mode {
            AccessMode::FullAccess => "shield-alert",
            AccessMode::Auto => "sparkles",
            AccessMode::AutoAcceptEdits => "pencil-edit",
            _ => "square-lock",
        };
        let plain = |id, label: SharedString| {
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(label.clone())
                .h(px(28.))
                .px_1p5()
                .mx(px(-6.))
                .rounded_md()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(12.))
                .text_color(content(0.85))
                .hover(|s| s.bg(content(0.06)))
                .child(label)
        };
        let identity = div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .w(px(160.))
            .flex_shrink_0()
            .child(
                div()
                    .id("bot-look")
                    .debug_selector(|| "bot-look".into())
                    .role(Role::Button)
                    .aria_label("Change mascot and color")
                    .relative()
                    .size(px(144.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_2xl()
                    .border_1()
                    .border_color(content(0.04))
                    .bg(content(0.04))
                    .hover(|s| s.bg(content(0.08)))
                    .child(self.settings_anchor.probe())
                    .child(avatar::settings_preview(
                        bot,
                        128.,
                        delete_anchor,
                        window,
                        cx,
                    ))
                    .on_click(
                        cx.listener(move |this, _, w, cx| this.open_settings_look(&look, w, cx)),
                    ),
            )
            .child(
                div()
                    .id("bot-name-field")
                    .debug_selector(|| "bot-name-field".into())
                    .mt_3()
                    .h(px(32.))
                    .w(px(160.))
                    .max_w_full()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .font_weight(FontWeight::MEDIUM)
                    .hover(|s| s.bg(content(0.05)))
                    .child(name),
            );
        let model_controls = card()
            .id("bot-model-settings")
            .debug_selector(|| "bot-model-settings".into())
            .flex_1()
            .min_w_0()
            .child(setting(
                "Model",
                None,
                div()
                    .id("bot-settings-model")
                    .role(Role::Button)
                    .aria_label(format!("Choose model, {model}"))
                    .h(px(28.))
                    .px_1p5()
                    .mx(px(-6.))
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(content(0.85))
                    .hover(|s| s.bg(content(0.06)))
                    .child(self.model_anchor.probe())
                    .child(provider_icon(&provider, 14.))
                    .child(div().min_w_0().truncate().child(model))
                    .on_click(cx.listener(|this, _, w, cx| this.open_model_picker(w, cx)))
                    .into_any_element(),
            ))
            .children(model_options.into_iter().filter_map(|item| {
                let MenuItem::Action(option) = item else {
                    return None;
                };
                let label = option.label.clone();
                let value = option
                    .value
                    .clone()
                    .unwrap_or_else(|| if option.checked { "On" } else { "Off" }.into());
                let anchor = AnchorSlot::default();
                let probe = anchor.probe();
                Some(
                    setting(
                        &label,
                        None,
                        plain(
                            SharedString::from(format!("bot-option-{}", option.id)),
                            value,
                        )
                        .child(probe)
                        .on_click(cx.listener(move |this, _, w, cx| {
                            if option.submenu.is_empty() {
                                if this.creating() {
                                    this.choose_creation_option(&option.id, cx);
                                } else if let Some(chat) = this.chat()
                                    && let Err(e) = chat
                                        .update(cx, |v, cx| v.bot_pick_model_option(&option.id, cx))
                                {
                                    this.error = Some(e);
                                }
                                this.changed(cx);
                            } else {
                                this.open_model_option(
                                    option.label.clone(),
                                    option.submenu.clone(),
                                    anchor.clone(),
                                    w,
                                    cx,
                                );
                            }
                        }))
                        .into_any_element(),
                    )
                    .border_t_1()
                    .border_color(content(0.04)),
                )
            }))
            .child(
                setting(
                    "Permissions",
                    None,
                    div()
                        .id("bot-settings-access")
                        .role(Role::Button)
                        .aria_label(format!("Permissions, {}", mode.label()))
                        .h(px(28.))
                        .max_w(px(208.))
                        .px_1p5()
                        .mx(px(-6.))
                        .rounded_md()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(px(12.))
                        .text_color(content(0.85))
                        .hover(|s| s.bg(content(0.06)))
                        .child(self.access_anchor.probe())
                        .child(
                            icon(access_icon, 14.).when(mode == AccessMode::FullAccess, |i| {
                                i.text_color(rgb(0xffb900))
                            }),
                        )
                        .child(mode.label())
                        .child(icon("chevron-down", 12.))
                        .on_click(cx.listener(|this, _, w, cx| this.open_access_picker(w, cx)))
                        .into_any_element(),
                )
                .border_t_1()
                .border_color(content(0.04)),
            );
        div().id("bot-general-settings").debug_selector(|| "bot-general-settings".into())
            .flex_1().min_h_0().overflow_y_scroll().when(narrow, |d| d.px_4()).when(!narrow, |d| d.px_8()).pt_6().pb_6()
            .child(div().text_size(px(14.)).font_weight(FontWeight::MEDIUM).child("General"))
            .child(div().id("bot-settings-header").debug_selector(|| "bot-settings-header".into()).mt_4().flex().gap_6()
                .when(narrow, |d| d.flex_col()).when(!narrow, |d| d.items_center()).child(identity).child(model_controls))
            .child(heading("Projects",Some("It can see every session, note and folder in these.")))
            .child(card()
                .children(bot.projects.iter().enumerate().map(|(ix,project)| {
                    let remove=project.clone(); let owner=bot.id.clone();
                    let appearance=self.owner.upgrade().and_then(|o|o.read(cx).workspace.project_appearance.get(project).cloned()).unwrap_or_default();
                    let label=appearance.label(project);
                    let pretty=std::env::var("HOME").ok().and_then(|home|project.strip_prefix(&home).map(|rest|format!("~{rest}"))).unwrap_or_else(||project.clone());
                    let group:SharedString=format!("bot-project-{ix}").into();
                    div().id(("bot-project",ix)).group(group.clone()).min_h(px(48.)).flex().items_center().gap_3().px_3().py_2()
                        .when(ix>0,|d|d.border_t_1().border_color(content(0.04)))
                        .child(div().size(px(32.)).flex_shrink_0().rounded_lg().bg(content(0.05)).flex().items_center().justify_center()
                            .child(crate::project_settings::project_icon(&appearance,project,16.)))
                        .child(div().flex_1().min_w_0()
                            .child(div().text_size(px(13.)).line_height(px(20.)).text_color(content(0.9)).truncate().child(label))
                            .child(div().text_size(px(11.)).line_height(px(16.)).text_color(content(0.4)).truncate().child(pretty)))
                        .child(icon_button("bot-remove-project","Remove project","close").opacity(0.).group_hover(group,|s|s.opacity(1.))
                            .on_click(cx.listener(move |this,_,_,cx| { this.edit_settings_bot(&owner, |bot| bot.projects.retain(|path| path != &remove), cx);})))
                }))
                .child(div().id("bot-add-project").role(Role::Button).aria_label("Add project").h(px(44.)).px_3().flex().items_center().gap_3()
                    .text_size(px(13.)).text_color(content(0.55)).hover(|s|s.bg(content(0.04)).text_color(content(1.)))
                    .when(!bot.projects.is_empty(),|d|d.border_t_1().border_color(content(0.04)))
                    .child(self.projects_anchor.probe())
                    .child(div().size(px(32.)).rounded_lg().border_1().border_color(content(0.15)).flex().items_center().justify_center().child(icon("plus",14.)))
                    .child("Add project").on_click(cx.listener(move |this,_,w,cx|this.pick_project(&add,w,cx)))))
            .child(heading("Sessions it starts",None))
            .child(card()
                .child(switch_setting("Show sessions in sidebar","Sessions it starts appear in your project sidebar. Hidden ones stay saved and open from its overview.",false,
                    self.switch("bot-show-started","Show sessions in sidebar",bot.show_started_sessions_in_sidebar,false,|m,on|m.show_started_sessions_in_sidebar=on,cx)))
                .child(switch_setting("Group in a folder",&if creating && bot.name.is_empty() { "Sessions it starts go in a folder named after this Bot.".into() } else { format!("Sessions it starts go in a “{}” folder in your project sidebar.",bot.label()) },!bot.show_started_sessions_in_sidebar,
                    self.switch("bot-group-started","Group in a folder",bot.show_started_sessions_in_sidebar&&bot.use_sidebar_folders,!bot.show_started_sessions_in_sidebar,|m,on|m.use_sidebar_folders=on,cx))
                    .border_t_1().border_color(content(0.04))))
            .into_any_element()
    }
    fn switch(
        &self,
        id: &'static str,
        label: &'static str,
        on: bool,
        disabled: bool,
        change: fn(&mut Bot, bool),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        crate::controls::switch(id, label, on, !disabled)
            .on_click(cx.listener(move |this, _, _, cx| {
                if disabled {
                    return;
                }
                if let Some(id) = this
                    .creation
                    .as_ref()
                    .map(|draft| draft.bot.id.clone())
                    .or(this.active.clone())
                {
                    this.edit_settings_bot(&id, |bot| change(bot, !on), cx);
                }
            }))
            .into_any_element()
    }
    /// BotMenu, beside the row's options button or at a right-click.
    pub(super) fn open_options(
        &mut self,
        id: &str,
        at: Option<gpui::Point<gpui::Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.minion_read_only(id) {
            return;
        }
        let Some(bot) = self.roster.get(id).cloned() else {
            return;
        };
        let editor = cx.new(|cx| LookEditor::new(bot, cx));
        let focus = editor.read(cx).name.focus_handle(cx);
        let changed = cx.subscribe(&editor, |this, _, event: &LookChanged, cx| {
            if let Some(bot) = this.roster.get_mut(&event.bot.id) {
                bot.name = event.bot.name.clone();
                bot.mascot = event.bot.mascot.clone();
                bot.shape = event.bot.shape.clone();
                bot.color = event.bot.color;
                bot.fg_color = event.bot.fg_color;
                bot.textured = event.bot.textured;
                bot.seed = event.bot.seed;
                bot.grid = event.bot.grid;
            }
            this.configure_chat(&event.bot.id, cx);
            this.save(cx);
        });
        let submitted = cx.subscribe(&editor, |this, _, _: &LookSubmitted, cx| {
            this.menu = None;
            cx.notify();
        });
        let menu = cx.new(|cx| {
            Menu::new(
                "Bot options",
                vec![
                    MenuItem::Custom(AnyView::from(editor), focus.clone()),
                    MenuItem::Separator,
                    MenuAction::new("background", "Background image")
                        .icon("image-add")
                        .into(),
                    MenuAction::new("settings", "Settings")
                        .icon("settings")
                        .into(),
                ],
                window,
                cx,
            )
            .width(280.)
            .style(MenuStyle::Group)
        });
        let target = id.to_string();
        let picked = cx.subscribe_in(&menu, window, move |this, _, event: &MenuEvent, w, cx| {
            this.menu = None;
            if let MenuEvent::Picked(action, _) = event {
                match action.as_ref() {
                    "background" => this.open_background(&target, w, cx),
                    "settings" => this.open_bot_settings(&target, w, cx),
                    _ => {}
                }
            }
            cx.notify();
        });
        self.menu_anchor = match at {
            Some(at) => Anchor::Point(at),
            None => self
                .rail_bounds
                .get(id)
                .cloned()
                .map(Anchor::Slot)
                .unwrap_or_else(|| Anchor::Slot(self.settings_anchor.clone())),
        };
        self.menu = Some((id.into(), menu, vec![changed, submitted, picked]));
        window.focus(&focus, cx);
        cx.notify();
    }
    pub(super) fn options_surface(&self, cx: &App) -> Option<AnyElement> {
        self.menu.as_ref().map(|(_, menu, _)| {
            menu_popover(menu, self.menu_anchor.clone(), cx)
                .without_animation()
                .side(Side::Right)
                .priority(90)
                .gap(0.)
                .into_any_element()
        })
    }
    pub(super) fn picker_surface(&self, cx: &App) -> Option<AnyElement> {
        self.picker.as_ref().map(|p| {
            menu_popover(&p.menu, p.anchor.clone(), cx)
                .align(p.align)
                .without_animation()
                .priority(90)
                .width(p.width)
                .into_any_element()
        })
    }
    fn open_model_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.creation.as_ref().map(|draft| draft.bot.id.clone());
        let (provider, model, project, account) = if let Some(draft) = &self.creation {
            (
                draft.provider,
                draft.settings.model.clone(),
                self.root.to_string_lossy().to_string(),
                draft.settings.account.clone(),
            )
        } else if let Some(chat) = self.chat() {
            let view = chat.read(cx);
            (
                Kind::from_id(&view.summary.provider),
                view.bot_model(),
                view.summary.cwd.clone(),
                view.settings.account.clone(),
            )
        } else {
            return;
        };
        let list = cx.new(|cx| ModelList::new_for(provider, model, &project, account, cx));
        let focus = list.read(cx).search_focus(cx);
        let picked = cx.subscribe(&list, move |this, _, picked: &ModelPicked, cx| {
            if let Some(target) = &target {
                let Some(draft) = this
                    .creation
                    .as_mut()
                    .filter(|draft| draft.bot.id == *target)
                else {
                    return;
                };
                draft.provider = picked.0;
                draft.settings.model = picked.1.clone();
                draft.settings.effort = crate::conversation::preferred_effort(picked.0.id(), cx);
                draft.settings.fast = false;
                draft.settings.thinking = None;
                draft.settings.service_tier = None;
                draft.settings.model_settings.clear();
                draft.catalog.clear();
                this.load_creation_catalog(cx);
            } else if let Some(view) = this.chat()
                && let Err(e) = view.update(cx, |v, cx| {
                    v.bot_config(picked.0, picked.1.clone(), HashMap::new(), cx)
                })
            {
                this.error = Some(e);
            }
            this.picker = None;
            this.changed(cx);
        });
        let menu = cx.new(|cx| {
            Menu::new(
                "Model",
                vec![MenuItem::Custom(list.into(), focus)],
                window,
                cx,
            )
            .width(400.)
        });
        let closed = cx.subscribe(&menu, |this, _, _: &MenuEvent, cx| {
            this.picker = None;
            cx.notify();
        });
        self.picker = Some(OpenPicker {
            menu,
            anchor: Anchor::Slot(self.model_anchor.clone()),
            width: 400.,
            align: crate::popover::Align::Start,
            _events: vec![picked, closed],
        });
        cx.notify();
    }
    fn open_model_option(
        &mut self,
        label: SharedString,
        choices: Vec<MenuAction>,
        anchor: AnchorSlot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.settings_identity();
        let menu = cx.new(|cx| {
            Menu::new(
                label,
                choices.into_iter().map(Into::into).collect(),
                window,
                cx,
            )
        });
        let picked = cx.subscribe(&menu, move |this, _, event: &MenuEvent, cx| {
            if this.settings_identity() != target {
                return;
            }
            if let MenuEvent::Picked(id, _) = event {
                if this.creating() {
                    this.choose_creation_option(id, cx);
                } else if let Some(chat) = this.chat()
                    && let Err(e) = chat.update(cx, |v, cx| v.bot_pick_model_option(id, cx))
                {
                    this.error = Some(e);
                }
            }
            this.picker = None;
            this.changed(cx);
        });
        self.picker = Some(OpenPicker {
            menu,
            anchor: Anchor::Slot(anchor),
            width: 228.,
            align: crate::popover::Align::Start,
            _events: vec![picked],
        });
        cx.notify();
    }
    fn open_access_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.settings_identity();
        let options = self
            .creation
            .as_ref()
            .map(|draft| {
                crate::conversation::bot_settings::access_options(
                    draft.provider,
                    draft.settings.access,
                    false,
                )
            })
            .or_else(|| self.chat().map(|chat| chat.read(cx).bot_access_options()))
            .unwrap_or_default();
        let menu = cx.new(|cx| {
            Menu::new("Permissions", options, window, cx)
                .style(MenuStyle::Access)
                .width(288.)
        });
        let picked = cx.subscribe(&menu, move |this, _, event: &MenuEvent, cx| {
            if this.settings_identity() != target {
                return;
            }
            if let MenuEvent::Picked(id, _) = event {
                let mode = AccessMode::from_id(id.strip_prefix("access:").unwrap_or(id));
                if let Some(draft) = &mut this.creation {
                    draft.settings.access = mode;
                } else if let Some(view) = this.chat() {
                    view.update(cx, |v, cx| v.bot_access(mode, cx));
                }
            }
            this.picker = None;
            cx.notify();
        });
        self.picker = Some(OpenPicker {
            menu,
            anchor: Anchor::Slot(self.access_anchor.clone()),
            width: 288.,
            align: crate::popover::Align::Start,
            _events: vec![picked],
        });
        cx.notify();
    }
    fn pick_project(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (projects, appearance) = self
            .owner
            .upgrade()
            .map(|owner| {
                let workspace = &owner.read(cx).workspace;
                (
                    workspace.projects.clone(),
                    workspace.project_appearance.clone(),
                )
            })
            .unwrap_or_default();
        let bot = id.to_string();
        let paths = projects.clone();
        let mut items: Vec<MenuItem> = projects
            .into_iter()
            .enumerate()
            .map(|(ix, path)| {
                let appearance = appearance.get(&path).cloned().unwrap_or_default();
                MenuAction::new(format!("project:{ix}"), appearance.label(&path))
                    .icon("folder")
                    .icon_color(rgb(appearance.color(&path)).into())
                    .description(path)
                    .into()
            })
            .collect();
        items.push(MenuItem::Separator);
        items.push(MenuAction::new("browse", "Open project…").into());
        let menu = cx.new(|cx| {
            Menu::new("Add project", items, window, cx)
                .style(MenuStyle::Composer)
                .width(300.)
        });
        let picked = cx.subscribe(&menu, move |this, _, event: &MenuEvent, cx| {
            this.picker = None;
            if let MenuEvent::Picked(action, _) = event {
                if action == "browse" {
                    this.browse_project(&bot, cx);
                } else if let Some(path) = action
                    .strip_prefix("project:")
                    .and_then(|n| n.parse::<usize>().ok())
                    .and_then(|ix| paths.get(ix))
                {
                    this.add_project(&bot, path.clone(), cx);
                }
            }
            cx.notify();
        });
        self.picker = Some(OpenPicker {
            menu,
            anchor: Anchor::Slot(self.projects_anchor.clone()),
            width: 300.,
            align: crate::popover::Align::Start,
            _events: vec![picked],
        });
        cx.notify();
    }
    fn add_project(&mut self, id: &str, path: String, cx: &mut Context<Self>) {
        self.edit_settings_bot(id, |bot| bot.projects.push(path), cx);
    }
    fn browse_project(&mut self, id: &str, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add project".into()),
        });
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |this, cx| {
                    this.add_project(&id, path.to_string_lossy().into(), cx)
                });
            }
        })
        .detach();
    }
    pub(super) fn open_bot_settings(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.minion_read_only(id) {
            return;
        }
        if self.roster.get(id).is_none() {
            return;
        }
        self.open(id, window, cx);
        self.settings = Some(SettingsSection::General);
        window.focus(&self.settings_focus, cx);
        self.reload_files(id, cx);
        cx.notify();
    }
    fn open_background(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bot) = self.roster.get(id).cloned() else {
            return;
        };
        let view = cx.new(|cx| {
            BackgroundDialog::new(
                format!("bot:{id}"),
                ProjectAppearance {
                    name: bot.label(),
                    color: Some(bot.color),
                    mascot: Some(bot.mascot),
                    background: bot.background,
                    ..Default::default()
                },
                cx,
            )
        });
        let id = id.to_string();
        let events = cx.subscribe(
            &view,
            move |this, _, event: &BackgroundEvent, cx| match event {
                BackgroundEvent::Changed(appearance) => {
                    if let Some(bot) = this.roster.get_mut(&id) {
                        bot.background = appearance.background.clone();
                    }
                    this.configure_chat(&id, cx);
                    this.save(cx);
                }
                BackgroundEvent::Close => {
                    this.background = None;
                    this.changed(cx);
                }
            },
        );
        window.focus(&view.focus_handle(cx), cx);
        self.background = Some((view, events));
        self.changed(cx);
    }
    pub(crate) fn close_background(&mut self, cx: &mut Context<Self>) {
        if self.background.take().is_some() {
            self.changed(cx);
        }
    }
    pub(crate) fn background_dialog(&self) -> Option<Entity<BackgroundDialog>> {
        self.background.as_ref().map(|(view, _)| view.clone())
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
