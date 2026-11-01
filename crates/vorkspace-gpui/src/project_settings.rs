//! TabGroupMenu's project appearance controls and ProjectBackgroundDialog.
use crate::{
    avatar::{BotLookConfig, cached_badge_or_bot_image, color_swatch_circle},
    color_picker::{ColorPicker, Picked},
    input::{self, Input},
    popover::{Align, Anchor, AnchorSlot, popover},
    project_background::ProjectBackground,
};
use gpui::{
    AnyElement, App, Context, Corners, Entity, EventEmitter, FocusHandle, Focusable, MouseButton,
    MouseDownEvent, MouseMoveEvent, ObjectFit, PathPromptOptions, Role, SharedString, Subscription,
    Window, canvas, div, img, prelude::*, px, rgb,
};
use std::path::PathBuf;
use vorkspace_core::project_appearance::{
    Background, BotAnimState, COLORS, Effect, ProjectAppearance, Scope, TEXTURE_PALETTES,
    default_foreground_color, import_image,
};
use vorkspace_ui::theme::*;

pub(crate) enum Event {
    Changed(ProjectAppearance),
    Close,
}

pub(crate) fn project_icon(appearance: &ProjectAppearance, project: &str, side: f32) -> AnyElement {
    if project.starts_with("remote://") && appearance.logo.is_none() {
        return icon("internet", side)
            .text_color(content(0.7))
            .into_any_element();
    }
    match &appearance.logo {
        Some(path) => img(PathBuf::from(path))
            .size(px(side))
            .object_fit(ObjectFit::Contain)
            .flex_shrink_0()
            .into_any_element(),
        None => {
            let pixel_side = ((side * 2.0).round() as u32).clamp(28, 96);
            let look = BotLookConfig {
                shape: "hex".into(),
                face: "pills".into(),
                one_eye: false,
                textured: appearance.textured,
                seed: appearance.seed(project),
                grid: appearance.grid(),
                bg_color: appearance.color(project),
                fg_color: appearance.fg_color(project),
            };
            canvas(
                |_, _, _| {},
                move |bounds, _, window, cx| {
                    let image = cached_badge_or_bot_image(
                        false,
                        &look,
                        false,
                        pixel_side,
                        BotAnimState::default(),
                        window,
                        cx,
                    );
                    let _ = window.paint_image(bounds, bounds, Corners::default(), image, 0, false);
                },
            )
            .size(px(side))
            .flex_shrink_0()
            .into_any_element()
        }
    }
}

fn button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
) -> gpui::Stateful<gpui::Div> {
    let label = label.into();
    let selector = label.clone();
    crate::controls::button(id, label.clone(), true)
        .debug_selector(move || format!("project-settings-{selector}"))
        .h(px(32.))
        .px(px(10.))
        .flex()
        .items_center()
        .justify_center()
        .gap_1p5()
        .rounded_md()
        .border_1()
        .border_color(content(0.1))
        .text_size(px(12.))
        .line_height(px(18.))
        .text_color(content(0.7))
        .hover(|s| s.bg(content(0.08)).text_color(content(1.)))
        .child(label)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditingProjectColor {
    Background,
    Foreground,
}

pub(crate) struct AppearanceControls {
    project: String,
    appearance: ProjectAppearance,
    name: Entity<Input>,
    dragging_grid: bool,
    grid_bounds: AnchorSlot,
    editing_color: Option<EditingProjectColor>,
    color_picker: Entity<ColorPicker>,
    busy: bool,
    error: Option<String>,
    _events: Vec<Subscription>,
}
impl EventEmitter<Event> for AppearanceControls {}
impl AppearanceControls {
    pub(crate) fn new(
        project: String,
        appearance: ProjectAppearance,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| {
            let mut field = Input::field("Project name", "Project name", content(0.35), cx);
            field.set_text(appearance.label(&project), cx);
            field.select_range(0..field.content.len(), cx);
            field
        });
        let initial_bg = appearance.color(&project);
        let color_picker = cx.new(|cx| ColorPicker::new(initial_bg, cx));
        let events = vec![
            cx.subscribe(&name, |this, _, _: &input::Changed, cx| {
                this.appearance.name = this.name.read(cx).content.to_string();
                this.save(cx);
            }),
            cx.subscribe(&name, |_, _, _: &input::Submitted, cx| {
                cx.emit(Event::Close)
            }),
            cx.subscribe(&color_picker, |this, _, event: &Picked, cx| {
                match this
                    .editing_color
                    .unwrap_or(EditingProjectColor::Background)
                {
                    EditingProjectColor::Background => this.appearance.color = Some(event.0),
                    EditingProjectColor::Foreground => this.appearance.fg_color = Some(event.0),
                }
                this.save(cx);
            }),
        ];
        Self {
            project,
            appearance,
            name,
            dragging_grid: false,
            grid_bounds: AnchorSlot::default(),
            editing_color: None,
            color_picker,
            busy: false,
            error: None,
            _events: events,
        }
    }
    pub(crate) fn focus(&self, cx: &App) -> FocusHandle {
        self.name.focus_handle(cx)
    }
    fn save(&mut self, cx: &mut Context<Self>) {
        self.appearance.normalize();
        cx.emit(Event::Changed(self.appearance.clone()));
        cx.notify();
    }
    fn randomize_seed(&mut self, cx: &mut Context<Self>) {
        let next = 10000
            + ((vorkspace_core::time::now_ms() as u32)
                .wrapping_mul(1664525)
                .wrapping_add(self.appearance.seed(&self.project))
                % 90000);
        self.appearance.seed = Some(next);
        self.save(cx);
    }
    fn set_grid_from_x(&mut self, x: gpui::Pixels, cx: &mut Context<Self>) {
        let Some(bounds) = self.grid_bounds.get() else {
            return;
        };
        let width = (bounds.size.width.as_f32() - 12.).max(1.);
        let t = ((x - bounds.left()).as_f32() - 6.) / width;
        let next = (2. + t.clamp(0., 1.) * 6.).round() as u8;
        if self.appearance.grid() != next {
            self.appearance.grid = Some(next);
            self.save(cx);
        }
    }
    fn toggle_color_picker(&mut self, target: EditingProjectColor, cx: &mut Context<Self>) {
        if self.editing_color == Some(target) {
            self.editing_color = None;
        } else {
            self.editing_color = Some(target);
            let c = match target {
                EditingProjectColor::Background => self.appearance.color(&self.project),
                EditingProjectColor::Foreground => self.appearance.fg_color(&self.project),
            };
            self.color_picker
                .update(cx, |picker, cx| picker.set_color(c, cx));
        }
        cx.notify();
    }
    fn choose_logo(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        cx.notify();
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose project logo".into()),
        });
        let project = self.project.clone();
        cx.spawn(async move |this, cx| {
            let result = match picker.await {
                Ok(Ok(Some(paths))) if !paths.is_empty() => {
                    let source = paths[0].clone();
                    cx.background_executor()
                        .spawn(async move {
                            import_image(
                                &source,
                                &crate::data_dir().join("project-images"),
                                &project,
                                true,
                            )
                            .map(Some)
                        })
                        .await
                }
                Ok(Ok(_)) => Ok(None),
                _ => Err("Could not open the image picker.".into()),
            };
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(Some(path)) => {
                        this.appearance.logo = Some(path);
                        this.save(cx);
                    }
                    Ok(None) => {}
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }
}
impl Render for AppearanceControls {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.appearance.color(&self.project);
        let current_fg = self.appearance.fg_color(&self.project);
        let textured = self.appearance.textured;
        let grid = self.appearance.grid();
        let fraction = (grid.saturating_sub(2) as f32 / 6.).clamp(0., 1.);
        let probe = self.grid_bounds.clone();
        let bg_active = self.editing_color == Some(EditingProjectColor::Background);
        let fg_active = self.editing_color == Some(EditingProjectColor::Foreground);
        div()
            .id("project-appearance-controls")
            .debug_selector(|| "project-appearance-controls".into())
            .role(Role::Group)
            .aria_label("Project appearance")
            .flex()
            .flex_col()
            .gap_2()
            .pb_2()
            .child(
                div()
                    .id("project-appearance-name")
                    .debug_selector(|| "project-appearance-name".into())
                    .h(px(34.))
                    .flex()
                    .items_center()
                    .px_2p5()
                    .rounded_lg()
                    .border_1()
                    .border_color(content(0.1))
                    .bg(content(0.05))
                    .child(self.name.clone()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px(px(2.))
                    .child(
                        div()
                            .id("choose-project-logo")
                            .debug_selector(|| "choose-project-logo".into())
                            .role(Role::Button)
                            .aria_label("Choose project logo")
                            .size(px(36.))
                            .flex_shrink_0()
                            .rounded_lg()
                            .border_1()
                            .border_color(content(0.1))
                            .bg(content(0.05))
                            .flex()
                            .items_center()
                            .justify_center()
                            .hover(|s| s.bg(content(0.1)))
                            .child(project_icon(&self.appearance, &self.project, 20.))
                            .on_click(cx.listener(|this, _, _, cx| this.choose_logo(cx))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(11.))
                            .line_height(px(16.5))
                            .text_color(content(0.5))
                            .child("Project logo")
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.))
                                    .line_height(px(18.))
                                    .text_color(content(0.7))
                                    .child(if self.busy {
                                        "Saving image…"
                                    } else {
                                        "Soft Hex · Shown in sidebar and tabs"
                                    }),
                            ),
                    )
                    .when(self.appearance.logo.is_some(), |d| {
                        d.child(
                            icon_button("remove-project-logo", "Remove project logo", "trash")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.appearance.logo = None;
                                    this.save(cx);
                                })),
                        )
                    }),
            )
            // Texture Toggle Switch
            .child(
                div()
                    .px(px(2.))
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
                            "project-texture-toggle",
                            "Texture",
                            textured,
                            true,
                        )
                        .debug_selector(|| "project-texture-toggle".into())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.appearance.textured = !this.appearance.textured;
                            if !this.appearance.textured
                                && this.editing_color == Some(EditingProjectColor::Foreground)
                            {
                                this.editing_color = None;
                            }
                            this.save(cx);
                        })),
                    ),
            )
            // Grid Scale Slider (2x2..8x8) + Seed Die shown only when Texture is enabled
            .when(textured, |d| {
                d.child(
                    div()
                        .px(px(2.))
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
                                                .id("project-randomize-seed")
                                                .debug_selector(|| "project-randomize-seed".into())
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
                                .id("project-grid-slider")
                                .debug_selector(|| "project-grid-slider".into())
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
                                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                        this.dragging_grid = true;
                                        this.set_grid_from_x(event.position.x, cx);
                                        cx.stop_propagation();
                                    }),
                                )
                                .on_mouse_move(cx.listener(
                                    |this, event: &MouseMoveEvent, _, cx| {
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
            // Background & Foreground Color Buttons
            .child(
                div()
                    .px(px(2.))
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
                                    .id("project-bg-color-button")
                                    .debug_selector(|| "project-bg-color-button".into())
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
                                    .child(color_swatch_circle(current, None, 10.))
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(content(0.85))
                                            .child(if textured { "Background" } else { "Color" }),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_color_picker(
                                            EditingProjectColor::Background,
                                            cx,
                                        );
                                    })),
                            )
                            .when(textured, |d| {
                                d.child(
                                    div()
                                        .id("project-fg-color-button")
                                        .debug_selector(|| "project-fg-color-button".into())
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
                                        .child(color_swatch_circle(current_fg, None, 10.))
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .text_color(content(0.85))
                                                .child("Foreground"),
                                        )
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.toggle_color_picker(
                                                EditingProjectColor::Foreground,
                                                cx,
                                            );
                                        })),
                                )
                                .child(
                                    div()
                                        .id("project-swap-colors")
                                        .debug_selector(|| "project-swap-colors".into())
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
                                            let bg = this.appearance.color(&this.project);
                                            let fg = this.appearance.fg_color(&this.project);
                                            this.appearance.color = Some(fg);
                                            this.appearance.fg_color = Some(bg);
                                            if let Some(target) = this.editing_color {
                                                let next_c = match target {
                                                    EditingProjectColor::Background => fg,
                                                    EditingProjectColor::Foreground => bg,
                                                };
                                                this.color_picker.update(cx, |picker, cx| {
                                                    picker.set_color(next_c, cx)
                                                });
                                            }
                                            this.save(cx);
                                        })),
                                )
                            }),
                    ),
            )
            // Preset Color Swatches (9 circular swatches, no custom picker button)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_1()
                    .px(px(2.))
                    .children(COLORS.into_iter().enumerate().map(|(ix, color)| {
                        let pair_fg = TEXTURE_PALETTES
                            .get(ix)
                            .map(|p| p.2)
                            .unwrap_or_else(|| default_foreground_color(color));
                        div()
                            .id(("project-color", ix))
                            .debug_selector(move || format!("project-color-{ix}"))
                            .role(Role::Button)
                            .aria_label(format!("Project color #{color:06x}"))
                            .aria_selected(current == color)
                            .size(px(22.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .when(current == color, |d| {
                                d.border_2().border_color(content(0.7))
                            })
                            .child(color_swatch_circle(color, textured.then_some(pair_fg), 16.))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.appearance.color = Some(color);
                                if this.appearance.textured {
                                    this.appearance.fg_color = Some(pair_fg);
                                }
                                this.editing_color = None;
                                this.save(cx);
                            }))
                    })),
            )
            .when(self.editing_color.is_some(), |d| {
                d.child(div().px(px(2.)).child(self.color_picker.clone()))
            })
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(0xff6467))
                        .child(error),
                )
            })
    }
}

pub(crate) struct BackgroundDialog {
    project: String,
    appearance: ProjectAppearance,
    background: Background,
    preview: Entity<ProjectBackground>,
    focus: FocusHandle,
    sliders: [FocusHandle; 2],
    dragging: Option<bool>,
    busy: bool,
    error: Option<String>,
    bounds: AnchorSlot,
    effect_anchor: AnchorSlot,
    effect_region: AnchorSlot,
    effect_open: bool,
    effect_active: usize,
    effect_focus: FocusHandle,
    effect_trigger_focus: FocusHandle,
    _preview_events: Subscription,
}
impl EventEmitter<Event> for BackgroundDialog {}
impl Focusable for BackgroundDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl BackgroundDialog {
    pub(crate) fn new(
        project: String,
        appearance: ProjectAppearance,
        cx: &mut Context<Self>,
    ) -> Self {
        let background = appearance.background.clone().unwrap_or_default();
        let preview = cx.new(|_| ProjectBackground::preview());
        let preview_events = cx.observe(&preview, |_, _, cx| cx.notify());
        Self {
            project,
            appearance,
            background,
            preview,
            _preview_events: preview_events,
            bounds: AnchorSlot::default(),
            effect_anchor: AnchorSlot::default(),
            effect_region: AnchorSlot::default(),
            effect_open: false,
            effect_active: 0,
            effect_focus: cx.focus_handle(),
            effect_trigger_focus: cx.focus_handle(),
            focus: cx.focus_handle(),
            sliders: [cx.focus_handle(), cx.focus_handle()],
            dragging: None,
            busy: false,
            error: None,
        }
    }
    fn save(&mut self, cx: &mut Context<Self>) {
        self.background.normalize();
        self.appearance.background =
            (!self.background.path.is_empty()).then(|| self.background.clone());
        cx.emit(Event::Changed(self.appearance.clone()));
        cx.notify();
    }
    fn choose(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        cx.notify();
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose background image".into()),
        });
        let project = self.project.clone();
        cx.spawn(async move |this, cx| {
            let result = match picker.await {
                Ok(Ok(Some(paths))) if !paths.is_empty() => {
                    let source = paths[0].clone();
                    cx.background_executor()
                        .spawn(async move {
                            import_image(
                                &source,
                                &crate::data_dir().join("project-images"),
                                &project,
                                false,
                            )
                            .map(Some)
                        })
                        .await
                }
                Ok(Ok(_)) => Ok(None),
                _ => Err("Could not open the image picker.".into()),
            };
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(Some(path)) => {
                        this.background.path = path;
                        this.save(cx);
                    }
                    Ok(None) => {}
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn set_opacity(&mut self, empty: bool, value: f32, cx: &mut Context<Self>) {
        let value = (value * 100.).round() / 100.;
        if empty {
            self.background.empty_opacity = value;
        } else {
            self.background.session_opacity = value;
        }
        self.save(cx);
    }
    pub(crate) fn measured_height(&self) -> Option<f32> {
        self.bounds.get().map(|bounds| bounds.size.height.as_f32())
    }
    pub(crate) fn popup_region(&self) -> AnchorSlot {
        self.effect_region.clone()
    }
    fn close_effect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.effect_open = false;
        window.focus(&self.effect_trigger_focus, cx);
        cx.notify();
    }
    fn toggle_effect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.effect_open {
            self.close_effect(window, cx);
            return;
        }
        self.effect_open = true;
        self.effect_active = Effect::ALL
            .iter()
            .position(|effect| *effect == self.background.effect)
            .unwrap_or(0);
        window.focus(&self.effect_focus, cx);
        cx.notify();
    }
    fn choose_effect(&mut self, effect: Effect, window: &mut Window, cx: &mut Context<Self>) {
        self.background.effect = effect;
        self.close_effect(window, cx);
        self.save(cx);
    }
    fn effect_popup(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.effect_open {
            return None;
        }
        let weak = cx.weak_entity();
        Some(
            popover(
                "project-background-effects",
                Anchor::Slot(self.effect_anchor.clone()),
            )
            .width(144.)
            .gap(4.)
            .align(Align::End)
            .padding(4.)
            .priority(70)
            .track(self.effect_region.clone())
            .on_dismiss(move |window, cx| {
                let _ = weak.update(cx, |this, cx| this.close_effect(window, cx));
            })
            .child(
                div()
                    .id("project-background-effect-options")
                    .role(Role::ListBox)
                    .aria_label("Project background effect options")
                    .track_focus(&self.effect_focus)
                    .flex()
                    .flex_col()
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                        match event.keystroke.key.as_str() {
                            "up" => this.effect_active = this.effect_active.saturating_sub(1),
                            "down" => {
                                this.effect_active =
                                    (this.effect_active + 1).min(Effect::ALL.len() - 1)
                            }
                            "enter" | "space" => {
                                this.choose_effect(Effect::ALL[this.effect_active], window, cx)
                            }
                            "escape" => this.close_effect(window, cx),
                            _ => return,
                        }
                        window.prevent_default();
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .children(Effect::ALL.into_iter().enumerate().map(|(ix, effect)| {
                        div()
                            .id(("background-effect-option", ix))
                            .debug_selector(move || format!("project-settings-{}", effect.label()))
                            .role(Role::ListBoxOption)
                            .aria_label(effect.label())
                            .aria_selected(self.background.effect == effect)
                            .h(px(32.))
                            .flex_shrink_0()
                            .px_2()
                            .rounded_md()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(13.))
                            .line_height(px(13.))
                            .text_color(content(if self.effect_active == ix { 1. } else { 0.75 }))
                            .when(self.effect_active == ix, |d| d.bg(content(0.1)))
                            .hover(|s| s.bg(content(0.05)).text_color(content(1.)))
                            .child(
                                div()
                                    .size(px(14.))
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .when(self.background.effect == effect, |d| {
                                        d.child(icon("check", 12.).text_color(content(1.)))
                                    }),
                            )
                            .child(effect.label())
                            .on_hover(cx.listener(move |this, hovering: &bool, _, cx| {
                                if *hovering {
                                    this.effect_active = ix;
                                    cx.notify();
                                }
                            }))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.choose_effect(effect, window, cx)
                            }))
                    })),
            )
            .into_any_element(),
        )
    }
    fn slider(&self, empty: bool, cx: &mut Context<Self>) -> AnyElement {
        let value = if empty {
            self.background.empty_opacity
        } else {
            self.background.session_opacity
        };
        let fraction = (value - 0.05) / 0.60;
        let bounds = AnchorSlot::default();
        let movement = bounds.clone();
        let probe = bounds.clone();
        let focus = self.sliders[usize::from(!empty)].clone();
        let label = if empty {
            "Empty chat visibility"
        } else {
            "Session visibility"
        };
        div()
            .debug_selector(move || {
                if empty {
                    "background-empty-row".into()
                } else {
                    "background-session-row".into()
                }
            })
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .pt_4()
            .border_t_1()
            .border_color(content(0.07))
            .child(
                div()
                    .text_size(px(13.))
                    .line_height(px(19.5))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(label),
            )
            .child(
                div()
                    .w(px(224.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .id(if empty {
                                "empty-background-opacity"
                            } else {
                                "session-background-opacity"
                            })
                            .debug_selector(move || {
                                if empty {
                                    "empty-background-opacity".into()
                                } else {
                                    "session-background-opacity".into()
                                }
                            })
                            .role(Role::Slider)
                            .aria_label(label)
                            .aria_value(format!("{}%", (value * 100.).round()))
                            .track_focus(&focus)
                            .tab_index(0)
                            .relative()
                            .w(px(172.))
                            .h(px(12.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .child(probe.probe())
                            .child(div().w_full().h(px(4.)).rounded_full().bg(content(0.15)))
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .left(px(fraction * 160.))
                                    .size(px(12.))
                                    .rounded_full()
                                    .bg(content(1.)),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                    window.focus(&this.sliders[usize::from(!empty)], cx);
                                    this.dragging = Some(empty);
                                    if let Some(bounds) = bounds.get() {
                                        this.set_opacity(
                                            empty,
                                            0.05 + ((event.position.x - bounds.left()).as_f32()
                                                - 6.)
                                                / (bounds.size.width.as_f32() - 12.)
                                                * 0.60,
                                            cx,
                                        );
                                    }
                                    cx.stop_propagation();
                                }),
                            )
                            .on_mouse_move(cx.listener(
                                move |this, event: &MouseMoveEvent, _, cx| {
                                    if this.dragging == Some(empty)
                                        && event.pressed_button == Some(MouseButton::Left)
                                        && let Some(bounds) = movement.get()
                                    {
                                        this.set_opacity(
                                            empty,
                                            0.05 + ((event.position.x - bounds.left()).as_f32()
                                                - 6.)
                                                / (bounds.size.width.as_f32() - 12.)
                                                * 0.60,
                                            cx,
                                        );
                                    }
                                },
                            ))
                            .on_key_down(cx.listener(
                                move |this, event: &gpui::KeyDownEvent, _, cx| {
                                    let old = if empty {
                                        this.background.empty_opacity
                                    } else {
                                        this.background.session_opacity
                                    };
                                    let next = match event.keystroke.key.as_str() {
                                        "left" | "down" => old - 0.01,
                                        "right" | "up" => old + 0.01,
                                        "home" => 0.05,
                                        "end" => 0.65,
                                        _ => return,
                                    };
                                    this.set_opacity(empty, next, cx);
                                    cx.stop_propagation();
                                },
                            )),
                    )
                    .child(
                        div()
                            .w(px(40.))
                            .text_right()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .font_features(crate::panel_kit::tabular())
                            .child(format!("{}%", (value * 100.).round())),
                    ),
            )
            .into_any_element()
    }
    fn effect_row(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .debug_selector(|| "background-effect-row".into())
            .flex_shrink_0()
            .pt_4()
            .border_t_1()
            .border_color(content(0.07))
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(13.))
                            // React's inline label sits in a 16px / 24px line box.
                            .line_height(px(24.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child("Background effect"),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(11.))
                            .line_height(px(16.5))
                            .text_color(content(0.45))
                            .child(self.background.effect.description()),
                    ),
            )
            .child(
                div()
                    .id("project-background-effect")
                    .debug_selector(|| "project-background-effect".into())
                    .role(Role::ComboBox)
                    .aria_label("Project background effect")
                    .aria_value(self.background.effect.label())
                    .aria_expanded(self.effect_open)
                    .track_focus(&self.effect_trigger_focus)
                    .tab_index(0)
                    .w(px(144.))
                    .h(px(36.))
                    .flex_shrink_0()
                    .px(px(10.))
                    .rounded_md()
                    .border_1()
                    .border_color(content(0.1))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .text_size(px(13.))
                    .line_height(px(19.5))
                    .hover(|s| s.border_color(content(0.2)))
                    .child(self.effect_anchor.probe())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(self.background.effect.label()),
                    )
                    .child(icon("chevron-down", 14.).text_color(content(0.45)))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_effect(window, cx)))
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space" | "down") {
                            this.toggle_effect(window, cx);
                            window.prevent_default();
                            cx.stop_propagation();
                        }
                    })),
            )
            .into_any_element()
    }
    fn scope_row(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .debug_selector(|| "background-scope-row".into())
            .flex_shrink_0()
            .pt_4()
            .border_t_1()
            .border_color(content(0.07))
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .child(
                div()
                    .text_size(px(13.))
                    .line_height(px(19.5))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child("Show on"),
            )
            .child(
                div()
                    .id("background-scope-control")
                    .debug_selector(|| "background-scope-control".into())
                    .w(px(176.))
                    .h(px(32.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .p(px(2.))
                    .rounded_md()
                    .border_1()
                    .border_color(content(0.1))
                    .role(Role::RadioGroup)
                    .aria_label("Show project background on")
                    .children(
                        [(Scope::Empty, "Empty only"), (Scope::All, "All sessions")]
                            .into_iter()
                            .enumerate()
                            .map(|(ix, (scope, label))| {
                                div()
                                    .id(("background-scope", ix))
                                    .debug_selector(move || format!("project-settings-{label}"))
                                    .role(Role::RadioButton)
                                    .aria_label(label)
                                    .aria_selected(self.background.scope == scope)
                                    .h_full()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .px(px(6.))
                                    .rounded(px(5.))
                                    .text_size(px(12.))
                                    .line_height(px(18.))
                                    .text_color(content(if self.background.scope == scope {
                                        1.
                                    } else {
                                        0.5
                                    }))
                                    .when(self.background.scope == scope, |d| d.bg(content(0.1)))
                                    .hover(|s| s.text_color(content(1.)))
                                    .child(label)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.background.scope = scope;
                                        this.save(cx);
                                    }))
                            }),
                    ),
            )
            .into_any_element()
    }
}
impl Render for BackgroundDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_image = !self.background.path.is_empty();
        let settings = has_image.then(|| self.background.clone());
        self.preview.update(cx, |preview, cx| {
            preview.configure(settings, true, None, cx)
        });
        let error = self
            .error
            .clone()
            .or_else(|| self.preview.read(cx).error.clone());
        div().id("project-background-dialog").debug_selector(||"project-background-dialog".into())
            .role(Role::Dialog).aria_label("Background Image").track_focus(&self.focus)
            .relative().min_h_0().max_h(px(window.viewport_size().height.as_f32()-34.))
            .flex().flex_col().text_color(content(1.))
            .child(self.bounds.layout_probe())
            .on_key_down(cx.listener(|this,event:&gpui::KeyDownEvent,window,cx| {
                if event.keystroke.key=="escape" {
                    if this.effect_open {this.close_effect(window,cx);} else {cx.emit(Event::Close);}
                    window.prevent_default();cx.stop_propagation();
                }
            }))
            .on_mouse_up(MouseButton::Left,cx.listener(|this,_,_,_|this.dragging=None))
            .child(div().debug_selector(||"background-dialog-header".into()).flex_shrink_0().flex().items_start().gap_2().px_4().pt_3()
                .child(div().min_w_0().flex_1().pt(px(2.)).flex().flex_col()
                    .child(div().debug_selector(||"background-dialog-title".into()).text_size(px(16.)).line_height(px(20.)).font_weight(gpui::FontWeight::SEMIBOLD).child("Background Image"))
                    .child(div().mt(px(2.)).min_w_0().truncate().text_size(px(12.)).line_height(px(16.5)).text_color(content(0.5)).child(format!("Choose a background image for {}",self.appearance.label(&self.project)))))
                .child(icon_button("close-project-background","Close background settings","close").debug_selector(|| "close-project-background".into()).size(px(28.)).on_click(cx.listener(|_,_,_,cx|cx.emit(Event::Close)))))
            .child(div().id("background-dialog-body").debug_selector(||"background-dialog-body".into())
                .min_h_0().overflow_y_scroll().flex().flex_col().gap_5().p_4()
                .child(div().flex_shrink_0().flex().flex_col()
                    .child(div().debug_selector(||"background-dialog-preview".into()).h(px(162.)).flex_shrink_0().overflow_hidden().rounded(px(12.)).border_1().border_color(content(0.1))
                        .bg(if self.background.effect==Effect::GradientBlur {rgb(0x171717).into()} else {content(0.05)})
                        .child(div().relative().h(px(160.)).child(self.preview.clone()).when(!has_image,|d|d.flex().items_center().justify_center().child(div().text_size(px(12.)).text_color(content(0.4)).child("No background selected")))))
                    .child(button("choose-project-background",if self.busy{"Saving image…"}else if has_image{"Change image"}else{"Choose image"})
                        .mt_2().flex_shrink_0().on_click(cx.listener(|this,_,_,cx|this.choose(cx))))
                    .child(div().mt(px(6.)).text_size(px(11.)).line_height(px(17.875)).text_color(content(0.45))
                        .child(if has_image{"This image overrides the global background for this project."}else{"No background selected for this project."}))
                    .when_some(error,|d,error|d.child(div().mt(px(6.)).text_size(px(12.)).line_height(px(18.)).text_color(rgb(0xff6467)).child(error))))
                .when(has_image,|d|d.child(self.effect_row(cx)))
                .child(self.scope_row(cx))
                .child(self.slider(true,cx)).child(self.slider(false,cx))
                .when(has_image,|d|d.child(button("remove-project-background","Remove background image").flex_shrink_0().text_color(rgb(0xff6467))
                    .on_click(cx.listener(|this,_,_,cx|{this.background=Background::default();this.effect_open=false;this.error=None;this.save(cx);}))))
            )
            .children(self.effect_popup(cx))
    }
}

#[cfg(test)]
mod control_tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};

    #[gpui::test]
    fn texture_switch_mouse_and_keyboard_preserve_name_and_clear_foreground_editor(
        cx: &mut TestAppContext,
    ) {
        let window = cx.update(|cx| {
            crate::input::bind(cx);
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| {
                    let mut view = AppearanceControls::new(
                        "/tmp/control-project".into(),
                        ProjectAppearance {
                            textured: true,
                            ..Default::default()
                        },
                        cx,
                    );
                    view.name
                        .update(cx, |input, cx| input.set_text("Unfinished name".into(), cx));
                    view.editing_color = Some(EditingProjectColor::Foreground);
                    view
                })
            })
            .unwrap()
        });
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let track = visual.debug_bounds("project-texture-toggle").unwrap();
        assert_eq!(track.size, gpui::size(px(36.), px(20.)));
        visual.simulate_click(track.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |view, window, cx| {
                assert!(!view.appearance.textured);
                assert!(view.editing_color.is_none());
                assert_eq!(view.name.read(cx).content, "Unfinished name");
                // The name field precedes the texture switch in this form's focus order.
                window.focus(&view.name.focus_handle(cx), cx);
                window.focus_next(cx);
            })
            .unwrap();
        visual.run_until_parked();
        visual.refresh().unwrap();
        crate::controls::tests::press_key(visual, "space");
        visual.run_until_parked();
        window
            .update(visual, |view, _, cx| {
                assert!(view.appearance.textured);
                assert_eq!(view.name.read(cx).content, "Unfinished name");
                assert!(view.editing_color.is_none());
            })
            .unwrap();
        assert_eq!(
            visual.debug_bounds("project-texture-toggle").unwrap(),
            track
        );
    }
}
