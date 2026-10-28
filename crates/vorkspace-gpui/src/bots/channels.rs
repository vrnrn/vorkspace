use super::*;
use crate::popover::{Anchor, AnchorSlot, Menu, MenuAction, MenuEvent, MenuItem, MenuStyle};
use gpui::{AnyView, EventEmitter, FocusHandle};
use serde_json::{Value, json};
use vorkspace_core::{
    bot::channels::{Channel, Channels},
    project_appearance::{COLORS, TEXTURE_PALETTES, default_foreground_color},
};
mod ui;
pub(super) const READ_KEY: &str = "bot-channel-read";

struct ChannelLookChanged {
    channel: Channel,
}
struct ChannelLookSubmitted;

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditingChannelColor {
    Background,
    Foreground,
}

struct ChannelLookEditor {
    channel: Channel,
    name: Entity<Input>,
    dragging_grid: bool,
    grid_bounds: AnchorSlot,
    editing_color: Option<EditingChannelColor>,
    color_picker: Entity<super::color_picker::ColorPicker>,
    _events: Vec<Subscription>,
}
impl EventEmitter<ChannelLookChanged> for ChannelLookEditor {}
impl EventEmitter<ChannelLookSubmitted> for ChannelLookEditor {}

impl ChannelLookEditor {
    fn new(channel: Channel, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| {
            let mut field = Input::field("Channel name", "Channel name", content(0.35), cx);
            field.set_font_size(13.);
            field.set_max_chars(40);
            field.set_text(channel.name.clone(), cx);
            field.select_range(0..field.content.len(), cx);
            field
        });
        let initial_bg = channel.resolved_color();
        let color_picker = cx.new(|cx| super::color_picker::ColorPicker::new(initial_bg, cx));
        let changed = cx.subscribe(&name, |this, _, _: &input::Changed, cx| {
            let next: String = this.name.read(cx).content.trim().chars().take(40).collect();
            if !next.is_empty() {
                this.channel.name = next;
                this.emit(cx);
            }
        });
        let submitted = cx.subscribe(&name, |_, _, _: &input::Submitted, cx| {
            cx.emit(ChannelLookSubmitted);
        });
        let picked = cx.subscribe(
            &color_picker,
            |this, _, event: &super::color_picker::Picked, cx| {
                match this
                    .editing_color
                    .unwrap_or(EditingChannelColor::Background)
                {
                    EditingChannelColor::Background => this.channel.color = Some(event.0),
                    EditingChannelColor::Foreground => this.channel.fg_color = Some(event.0),
                }
                this.emit(cx);
            },
        );
        Self {
            channel,
            name,
            dragging_grid: false,
            grid_bounds: AnchorSlot::default(),
            editing_color: None,
            color_picker,
            _events: vec![changed, submitted, picked],
        }
    }

    fn focus(&self, cx: &App) -> FocusHandle {
        self.name.focus_handle(cx)
    }

    fn emit(&mut self, cx: &mut Context<Self>) {
        cx.emit(ChannelLookChanged {
            channel: self.channel.clone(),
        });
        cx.notify();
    }

    fn randomize_seed(&mut self, cx: &mut Context<Self>) {
        let next = 10000
            + ((vorkspace_core::time::now_ms() as u32)
                .wrapping_mul(1664525)
                .wrapping_add(self.channel.resolved_seed())
                % 90000);
        self.channel.seed = Some(next);
        self.emit(cx);
    }

    fn set_grid_from_x(&mut self, x: gpui::Pixels, cx: &mut Context<Self>) {
        let Some(bounds) = self.grid_bounds.get() else {
            return;
        };
        let width = (bounds.size.width.as_f32() - 12.).max(1.);
        let t = ((x - bounds.left()).as_f32() - 6.) / width;
        let next = (2. + t.clamp(0., 1.) * 6.).round() as u8;
        if self.channel.resolved_grid() != next {
            self.channel.grid = Some(next);
            self.emit(cx);
        }
    }

    fn toggle_color_picker(&mut self, target: EditingChannelColor, cx: &mut Context<Self>) {
        if self.editing_color == Some(target) {
            self.editing_color = None;
        } else {
            self.editing_color = Some(target);
            let c = match target {
                EditingChannelColor::Background => self.channel.resolved_color(),
                EditingChannelColor::Foreground => self.channel.resolved_fg_color(),
            };
            self.color_picker
                .update(cx, |picker, cx| picker.set_color(c, cx));
        }
        cx.notify();
    }
}

impl gpui::Render for ChannelLookEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let textured = self.channel.textured;
        let grid = self.channel.resolved_grid();
        let fraction = (grid.saturating_sub(2) as f32 / 6.).clamp(0., 1.);
        let probe = self.grid_bounds.clone();
        let current_bg = self.channel.resolved_color();
        let current_fg = self.channel.resolved_fg_color();
        let bg_active = self.editing_color == Some(EditingChannelColor::Background);
        let fg_active = self.editing_color == Some(EditingChannelColor::Foreground);
        div()
            .id("channel-look-editor")
            .debug_selector(|| "channel-look-editor".into())
            .flex()
            .flex_col()
            .gap_2()
            .pb_1()
            .child(
                div()
                    .id("channel-look-name")
                    .debug_selector(|| "channel-look-name".into())
                    .mx_1()
                    .mt_1()
                    .h(px(34.))
                    .px_2p5()
                    .flex()
                    .items_center()
                    .rounded_md()
                    .border_1()
                    .border_color(content(0.1))
                    .bg(content(0.05))
                    .child(self.name.clone()),
            )
            // Texture Toggle Switch
            .child(
                div()
                    .px_1()
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
                            "channel-texture-toggle",
                            "Texture",
                            textured,
                            true,
                        )
                        .debug_selector(|| "channel-texture-toggle".into())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.channel.textured = !this.channel.textured;
                            if !this.channel.textured
                                && this.editing_color == Some(EditingChannelColor::Foreground)
                            {
                                this.editing_color = None;
                            }
                            this.emit(cx);
                        })),
                    ),
            )
            // Grid Scale Slider + Seed Die when textured
            .when(textured, |d| {
                d.child(
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
                                                .id("channel-seed-randomize")
                                                .debug_selector(|| "channel-seed-randomize".into())
                                                .role(Role::Button)
                                                .aria_label("Randomize seed")
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
                                                    this.randomize_seed(cx)
                                                })),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .id("channel-grid-slider")
                                .debug_selector(|| "channel-grid-slider".into())
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
            // Background & Foreground Color Buttons
            .child(
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
                                    .id("channel-bg-color-button")
                                    .debug_selector(|| "channel-bg-color-button".into())
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
                                        this.toggle_color_picker(
                                            EditingChannelColor::Background,
                                            cx,
                                        );
                                    })),
                            )
                            .when(textured, |d| {
                                d.child(
                                    div()
                                        .id("channel-fg-color-button")
                                        .debug_selector(|| "channel-fg-color-button".into())
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
                                            this.toggle_color_picker(
                                                EditingChannelColor::Foreground,
                                                cx,
                                            );
                                        })),
                                )
                                .child(
                                    div()
                                        .id("channel-swap-colors")
                                        .debug_selector(|| "channel-swap-colors".into())
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
                                            let bg = this.channel.resolved_color();
                                            let fg = this.channel.resolved_fg_color();
                                            this.channel.color = Some(fg);
                                            this.channel.fg_color = Some(bg);
                                            if let Some(target) = this.editing_color {
                                                let next_c = match target {
                                                    EditingChannelColor::Background => fg,
                                                    EditingChannelColor::Foreground => bg,
                                                };
                                                this.color_picker.update(cx, |picker, cx| {
                                                    picker.set_color(next_c, cx)
                                                });
                                            }
                                            this.emit(cx);
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
                    .gap(px(2.))
                    .px_1()
                    .children(COLORS.into_iter().enumerate().map(|(ix, color)| {
                        let fg_pair = TEXTURE_PALETTES
                            .get(ix)
                            .map(|p| p.2)
                            .unwrap_or_else(|| default_foreground_color(color));
                        let selected = current_bg == color;
                        div()
                            .id(("channel-color", ix))
                            .debug_selector(move || format!("channel-color-{ix}"))
                            .role(Role::Button)
                            .aria_label(format!("Color #{color:06x}"))
                            .aria_selected(selected)
                            .size(px(22.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .when(selected, |d| d.border_2().border_color(content(0.8)))
                            .child(avatar::color_swatch_circle(
                                color,
                                textured.then_some(fg_pair),
                                16.,
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.channel.color = Some(color);
                                if this.channel.textured {
                                    this.channel.fg_color = Some(fg_pair);
                                }
                                this.editing_color = None;
                                this.emit(cx);
                            }))
                    })),
            )
            .when(self.editing_color.is_some(), |d| {
                d.child(div().px_1().child(self.color_picker.clone()))
            })
    }
}

#[derive(Default)]
pub(super) struct ChannelUi {
    pub state: Channels,
    pub read_through: HashMap<String, String>,
    pub load_error: Option<String>,
    pub open: bool,
    pub inspecting: Option<(String, String)>,
    pub selected: Option<String>,
    pub views: HashMap<(String, String), Entity<Conversation>>,
    pub(super) loading: HashSet<(String, String)>,
    pub(super) starts: HashMap<String, usize>,
    pub(super) editing: bool,
    pub(super) settings_anchor: AnchorSlot,
    pub(super) rail_bounds: HashMap<String, AnchorSlot>,
    pub(super) completions: HashMap<String, Entity<crate::composer_completion::Completion>>,
    pub(super) form_members: HashMap<String, HashSet<String>>,
    pub(super) message_cache: HashMap<String, std::sync::Arc<Vec<crate::markdown::MarkdownRow>>>,
    pub(super) scroll: HashMap<String, gpui::ScrollHandle>,
}

impl Bots {
    fn channel_unread(&self, id: &str) -> bool {
        self.channels
            .state
            .get(id)
            .and_then(|c| c.messages.last())
            .and_then(|m| m["id"].as_str())
            .is_some_and(|last| {
                self.channels.read_through.get(id).map(String::as_str) != Some(last)
            })
    }

    fn mark_channel_read(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.channel_unread(id) {
            return;
        }
        let Some(last) = self
            .channels
            .state
            .get(id)
            .and_then(|c| c.messages.last())
            .and_then(|m| m["id"].as_str())
        else {
            return;
        };
        let mut next = self.channels.read_through.clone();
        next.retain(|id, _| self.channels.state.get(id).is_some());
        next.insert(id.into(), last.into());
        let saved = serde_json::to_string(&next)
            .map_err(|e| e.to_string())
            .and_then(|raw| locked(&self.store)?.save_setting(READ_KEY, &raw));
        match saved {
            Ok(()) => {
                self.channels.read_through = next;
                self.changed(cx);
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn mark_visible_channel_read(&mut self, cx: &mut Context<Self>) {
        if self.channels.open
            && !self.channels.editing
            && self.channels.inspecting.is_none()
            && self.parent_window.is_some()
            && self.parent_window == cx.active_window()
            && let Some(id) = self.channels.selected.clone()
        {
            self.mark_channel_read(&id, cx);
        }
    }

    pub(crate) fn classic_channels_open(&self) -> bool {
        self.prefs.bots && self.channels.open
    }

    pub(crate) fn close_classic_channels(&mut self, cx: &mut Context<Self>) {
        if self.channels.open {
            self.channels.open = false;
            self.channels.inspecting = None;
            cx.notify();
        }
    }

    /// The original rail lists Channels beside Bots, regardless of which
    /// project's session/file pane happens to be selected.
    pub(crate) fn classic_channels_rail(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if !self.prefs.bots {
            return div().into_any_element();
        }
        let channels = self.channels.state.channels.clone();
        for channel in &channels {
            self.channels
                .rail_bounds
                .entry(channel.id.clone())
                .or_default();
        }
        div()
            .id("classic-channels")
            .debug_selector(|| "classic-channels".into())
            .px_2()
            .pb_3()
            .child(
                section_label("Channels").child(
                    icon_button("classic-add-channel", "New channel", "plus")
                        .debug_selector(|| "classic-add-channel".into())
                        .focusable()
                        .tab_stop(true)
                        .size(px(20.))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_classic_channel(None, window, cx);
                        })),
                ),
            )
            .children(channels.into_iter().map(|channel| {
                let id = channel.id.clone();
                let selector = id.clone();
                let options = id.clone();
                let context = id.clone();
                let slot = self.channels.rail_bounds[&id].clone();
                let selected = self.channels.open
                    && self.channels.selected.as_deref() == Some(channel.id.as_str());
                let unread = self.channel_unread(&id);
                let seed = channel.resolved_seed();
                let grid = channel.resolved_grid();
                let textured = channel.textured;
                let bg = channel.resolved_color();
                let fg = channel.resolved_fg_color();
                div()
                    .id(SharedString::from(format!("classic-channel-{id}")))
                    .debug_selector(move || format!("classic-channel-{selector}"))
                    .group("classic-channel-row")
                    .role(Role::Button)
                    .aria_label(format!(
                        "Channel #{}{}",
                        channel.name,
                        if unread { ", unread messages" } else { "" }
                    ))
                    .aria_selected(selected)
                    .focusable()
                    .tab_stop(true)
                    .relative()
                    .overflow_hidden()
                    .h(px(32.))
                    .min_w_0()
                    .px_2()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(14.))
                    .opacity(if selected { 1. } else { 0.65 })
                    .when(selected, |d| {
                        d.child(avatar::sidebar_selection_pill(seed, grid, textured, bg, fg))
                    })
                    .hover(|s| s.bg(content(0.08)).opacity(1.))
                    .focus(|s| s.bg(content(0.08)).opacity(1.))
                    .child(
                        div()
                            .debug_selector({
                                let id = id.clone();
                                move || format!("channel-label-{id}")
                            })
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .group_hover("classic-channel-row", |s| s.pr(px(24.)))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(format!("# {}", channel.name)),
                    )
                    .when(unread, |d| {
                        d.child(
                            div()
                                .debug_selector({
                                    let id = id.clone();
                                    move || format!("channel-unread-{id}")
                                })
                                .size(px(6.))
                                .flex_shrink_0()
                                .rounded_full()
                                .bg(accent_color())
                                .group_hover("classic-channel-row", |s| s.invisible()),
                        )
                    })
                    .child(
                        div()
                            .absolute()
                            .right_2()
                            .top(px(6.))
                            .invisible()
                            .group_hover("classic-channel-row", |s| s.visible())
                            .child(
                                icon_button(
                                    "channel-options",
                                    "Channel options",
                                    "more-horizontal",
                                )
                                .debug_selector({
                                    let id = id.clone();
                                    move || format!("classic-channel-options-{id}")
                                })
                                .size(px(20.))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(
                                    move |this, _, w, cx| {
                                        cx.stop_propagation();
                                        this.open_channel_options(&options, None, w, cx);
                                    },
                                )),
                            ),
                    )
                    .child(slot.probe())
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, w, cx| {
                            this.open_channel_options(&context, Some(event.position), w, cx);
                        }),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_classic_channel(Some(id.clone()), window, cx);
                    }))
            }))
            .into_any_element()
    }

    pub(super) fn open_channel_options(
        &mut self,
        id: &str,
        at: Option<gpui::Point<gpui::Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(channel) = self.channels.state.get(id).cloned() else {
            return;
        };
        let editor = cx.new(|cx| ChannelLookEditor::new(channel, cx));
        let focus = editor.read(cx).focus(cx);
        let changed = cx.subscribe(&editor, |this, _, event: &ChannelLookChanged, cx| {
            let mut next = this.channels.state.clone();
            if let Some(ch) = next.get_mut(&event.channel.id) {
                ch.name = event.channel.name.clone();
                ch.color = event.channel.color;
                ch.fg_color = event.channel.fg_color;
                ch.textured = event.channel.textured;
                ch.seed = event.channel.seed;
                ch.grid = event.channel.grid;
            }
            if let Err(error) = this.commit_channels(next, cx) {
                this.error = Some(error);
                cx.notify();
            }
        });
        let submitted = cx.subscribe(&editor, |this, _, _: &ChannelLookSubmitted, cx| {
            this.menu = None;
            cx.notify();
        });
        let menu = cx.new(|cx| {
            Menu::new(
                "Channel options",
                vec![
                    MenuItem::Custom(AnyView::from(editor), focus.clone()),
                    MenuItem::Separator,
                    MenuAction::new("settings", "Channel settings")
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
            if let MenuEvent::Picked(action, _) = event
                && action.as_ref() == "settings"
            {
                this.open_classic_channel(Some(target.clone()), w, cx);
                this.channels.editing = true;
            }
            cx.notify();
        });
        self.menu_anchor = match at {
            Some(at) => Anchor::Point(at),
            None => self
                .channels
                .rail_bounds
                .get(id)
                .cloned()
                .map(Anchor::Slot)
                .unwrap_or_else(|| Anchor::Slot(self.channels.settings_anchor.clone())),
        };
        self.menu = Some((
            format!("channel:{id}"),
            menu,
            vec![changed, submitted, picked],
        ));
        window.focus(&focus, cx);
        cx.notify();
    }

    fn open_classic_channel(
        &mut self,
        id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.channels.open = true;
        self.channels.editing = false;
        if self.channels.selected != id {
            self.channels.message_cache.clear();
        }
        self.channels.selected = id;
        self.channels.inspecting = None;
        self.mark_visible_channel_read(cx);
        self.changed(cx);
        let owner = self.owner.clone();
        let manager = cx.weak_entity();
        window.defer(cx, move |window, cx| {
            let _ = owner.update(cx, |shell, cx| shell.open_classic_channels(window, cx));
            let _ = manager.update(cx, |this, cx| {
                if this.channels.selected.is_some() {
                    this.focus_channel_composer(window, cx);
                } else {
                    let name =
                        this.field("channel-new:name", "Channel name", String::new(), false, cx);
                    window.focus(&name.focus_handle(cx), cx);
                }
            });
        });
    }

    pub(super) fn stop_channel_delivery(
        &mut self,
        channel: &str,
        delivery: &str,
        cx: &mut Context<Self>,
    ) {
        let mut next = self.channels.state.clone();
        let Some(d) = next
            .get_mut(channel)
            .and_then(|c| c.deliveries.iter_mut().find(|d| d.id == delivery))
        else {
            return;
        };
        let running = d.state == "running";
        let key = (channel.to_string(), d.bot.clone());
        d.state = "interrupted".into();
        d.error = Some("Stopped by you".into());
        // A failed persistence write must never prevent Stop. The stored running
        // receipt is conservatively interrupted on recovery, never replayed.
        if let Err(error) = self.commit_channels(next.clone(), cx) {
            self.channels.state = next;
            self.error = Some(error);
        }
        if running && let Some(view) = self.channels.views.get(&key) {
            view.update(cx, |v, cx| v.bot_stop(cx));
        }
        cx.notify();
    }

    pub(super) fn commit_channels(
        &mut self,
        next: Channels,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if let Some(error) = &self.channels.load_error {
            return Err(error.clone());
        }
        next.save(&*locked(&self.store)?)?;
        self.channels.state = next;
        self.mark_visible_channel_read(cx);
        self.changed(cx);
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn channel_action(
        &mut self,
        action: &str,
        input: &Value,
        author: Option<&Bot>,
        request: &str,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        self.channel_action_with_hop(action, input, author, request, 0, cx)
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn channel_action_with_hop(
        &mut self,
        action: &str,
        input: &Value,
        author: Option<&Bot>,
        request: &str,
        hop: u8,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        if let Some(error) = &self.channels.load_error {
            return Err(error.clone());
        }
        bot::api::validate(action, input)?;
        let mut next = self.channels.state.clone();
        let result = match action {
            "channels.list" => json!(next.channels.iter().filter(|c| author.is_none_or(|m| c.member_ids.contains(&m.id))).map(|c| json!({"id":c.id,"name":c.name,"topic":c.topic,"memberIds":c.member_ids})).collect::<Vec<_>>()),
            "channels.create" => {
                let mut members = input["members"].as_array().ok_or("Provide member Bot names")?.iter().map(|v| v.as_str().map(str::to_owned).ok_or("Invalid member".to_string())).collect::<Result<Vec<_>,_>>()?;
                if let Some(m) = author { members.push(m.id.clone()); }
                let id = next.create(input["name"].as_str().ok_or("Provide name")?,input["topic"].as_str().unwrap_or(""), members, &self.roster)?;
                if let Some(author)=author && self.channels.state.get(&id).is_some_and(|c|!c.member_ids.contains(&author.id)) {return Err("This Bot is not a channel member".into());}
                json!({"id":id})
            },
            "channels.read" | "channels.post" => {
                let id = input["channel"].as_str().ok_or("Provide channel")?;
                let channel = next.get(id).ok_or("Unknown channel")?;
                if author.is_some_and(|m| !channel.member_ids.contains(&m.id)) { return Err("This Bot is not a channel member".into()); }
                if action == "channels.read" {
                    let limit = input["limit"].as_u64().unwrap_or(20).min(50) as usize;
                    let rows = channel.messages.iter().rev().take(limit).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>();
                    json!(rows)
                } else {
                    // A channel worker publishes exactly once through its final
                    // reply. Reject recursive same-turn posts through the CLI.
                    let targets = input["to"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect::<Vec<_>>();
                    let message = next.post(id,author,input["message"].as_str().ok_or("Provide message")?,request,hop,&targets,&self.roster)?;
                    json!({"accepted":true,"messageId":message})
                }
            },
            _ => return Err("Unknown channel action".into()),
        };
        if !matches!(action, "channels.list" | "channels.read") {
            self.commit_channels(next, cx)?;
        }
        Ok(result)
    }
    pub(super) fn channel_tick(&mut self, cx: &mut Context<Self>) {
        if self.channels.load_error.is_some() {
            return;
        }
        if let Some(key) = self.channels.inspecting.clone()
            && !self.channels.views.contains_key(&key)
            && let Some(bot) = self.roster.get(&key.1).cloned()
        {
            self.load_channel_view(key, bot, cx);
        }
        let queued = self
            .channels
            .state
            .channels
            .iter()
            .flat_map(|c| {
                c.deliveries
                    .iter()
                    .filter(|d| d.state == "queued")
                    .map(|d| (c.id.clone(), d.clone()))
            })
            .collect::<Vec<_>>();
        for (channel, delivery) in queued {
            let key = (channel.clone(), delivery.bot.clone());
            if self.channels.state.get(&channel).is_some_and(|c| {
                c.deliveries
                    .iter()
                    .any(|d| d.bot == delivery.bot && d.state == "running")
            }) {
                continue;
            }
            let Some(bot) = self.roster.get(&delivery.bot).cloned() else {
                continue;
            };
            let Some(view) = self.channels.views.get(&key).cloned() else {
                self.load_channel_view(key, bot, cx);
                continue;
            };
            if !view.read(cx).bot_configuration_ready() || view.read(cx).busy {
                view.update(cx, |view, cx| {
                    if view.needs_history_restore() {
                        view.load(cx);
                    }
                });
                continue;
            }
            let Some(primary) = self
                .chats
                .get(&bot.id)
                .filter(|v| v.read(cx).bot_configuration_ready())
            else {
                self.ensure_chat(&bot.id, cx);
                if let Some(primary) = self.chats.get(&bot.id) {
                    primary.update(cx, |view, cx| {
                        if view.needs_history_restore() {
                            view.load(cx);
                        }
                    });
                }
                continue;
            };
            let configuration = primary.read(cx).bot_execution_settings();
            let provider = Kind::from_id(&primary.read(cx).summary.provider);
            let mut context = bot.clone();
            // This lane owns its channel turn, not the primary Bot's queue.
            context.outbox.clear();
            let root = self.root.clone();
            let environment = self
                .bridge
                .as_ref()
                .map(|b| b.environment(view.read(cx).summary.id))
                .unwrap_or_default();
            view.update(cx, |v, cx| v.configure_bot(context, root, environment, cx));
            if !view.update(cx, |view, cx| {
                view.inherit_bot_configuration(provider, &configuration, cx)
            }) {
                continue;
            }
            let prompt = self
                .channels
                .state
                .get(&channel)
                .unwrap()
                .prompt(&delivery, &self.roster);
            let mut next = self.channels.state.clone();
            if let Some(d) = next
                .get_mut(&channel)
                .and_then(|c| c.deliveries.iter_mut().find(|d| d.id == delivery.id))
            {
                d.state = "running".into();
            }
            if let Err(error) = self.commit_channels(next, cx) {
                self.error = Some(error);
                break;
            }
            self.channels
                .starts
                .insert(delivery.id.clone(), view.read(cx).bot_rows(cx).len());
            let message = Message {
                prepared_prompt: None,
                peer: None,
                id: delivery.id.clone(),
                text: prompt,
                attachments: Vec::new(),
                internal: true,
                sent_at: Some(vorkspace_core::time::now_ms()),
                error: None,
            };
            view.update(cx, |v, cx| {
                v.bot_dispatch(&message, cx);
            });
        }
    }
    fn load_channel_view(&mut self, key: (String, String), bot: Bot, cx: &mut Context<Self>) {
        if !self.channels.loading.insert(key.clone()) {
            return;
        }
        self.ensure_chat(&bot.id, cx);
        let Some(primary) = self.chats.get(&bot.id) else {
            self.channels.loading.remove(&key);
            return;
        };
        if !primary.read(cx).bot_configuration_ready() {
            primary.update(cx, |view, cx| {
                if view.needs_history_restore() {
                    view.load(cx);
                }
            });
            self.channels.loading.remove(&key);
            return;
        }
        let mut settings = primary.read(cx).bot_execution_settings();
        settings.bot = None;
        settings.turns.clear();
        settings.environment.clear();
        settings.attachments.clear();
        let provider = primary.read(cx).summary.provider.clone();
        let saved = self
            .channels
            .state
            .get(&key.0)
            .and_then(|c| c.member_sessions.get(&key.1))
            .copied();
        let store = self.store.clone();
        let cwd = primary.read(cx).summary.cwd.clone();
        let work = cx.background_executor().spawn(async move {
            let db = locked(&store)?;
            if let Some(id) = saved {
                db.bot_session(id)?
                    .ok_or("Saved channel conversation is unavailable".into())
            } else {
                let summary = db.create(&provider, &cwd)?;
                db.configure_agent(summary.id, &provider, &settings)?;
                Ok(summary)
            }
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.channels.loading.remove(&key);
                let summary = match result {
                    Ok(s) => s,
                    Err(e) => {
                        this.error = Some(e);
                        cx.notify();
                        return;
                    }
                };
                let mut next = this.channels.state.clone();
                let valid = this.roster.get(&key.1).is_some()
                    && next.get(&key.0).is_some_and(|c| {
                        (c.member_ids.contains(&key.1)
                            || (saved.is_some() && this.channels.inspecting.as_ref() == Some(&key)))
                            && c.member_sessions.get(&key.1).copied() == saved
                    });
                if !valid {
                    // Only this task owns a newly created, unpublished session.
                    // Existing backing history is never deleted during a race.
                    if saved.is_none() {
                        let _ = locked(&this.store).and_then(|mut db| db.delete(summary.id));
                    }
                    return;
                }
                let channel = next.get_mut(&key.0).unwrap();
                channel.member_sessions.insert(key.1.clone(), summary.id);
                if let Err(e) = this.commit_channels(next, cx) {
                    this.error = Some(e);
                    if saved.is_none() {
                        let _ = locked(&this.store).and_then(|mut db| db.delete(summary.id));
                    }
                    return;
                }
                let id = summary.id;
                let view = cx.new(|cx| Conversation::new(summary, this.store.clone(), cx));
                let env = this
                    .bridge
                    .as_ref()
                    .map(|b| b.environment(id))
                    .unwrap_or_default();
                let root = this.root.clone();
                let mut bot = this.roster.get(&key.1).cloned().unwrap();
                bot.outbox.clear();
                view.update(cx, |v, cx| {
                    v.configure_bot(bot, root, env, cx);
                    v.load(cx);
                });
                let observed = key.clone();
                this.observations.push(
                    cx.subscribe(&view, move |this, _, _: &SessionChanged, cx| {
                        this.channel_finished(&observed, cx)
                    }),
                );
                this.observations.push(cx.observe(&view, |this, view, cx| {
                    if view.read(cx).bot_ready() {
                        this.channel_tick(cx);
                    }
                }));
                this.channels.views.insert(key, view);
                this.roster.hidden_sessions.push(id);
                this.save(cx);
                this.channel_tick(cx);
            });
        })
        .detach();
    }
    pub(super) fn channel_finished(&mut self, key: &(String, String), cx: &mut Context<Self>) {
        let Some(view) = self.channels.views.get(key) else {
            return;
        };
        let view = view.read(cx);
        if view.busy {
            cx.notify();
            return;
        }
        let Some(delivery) = self
            .channels
            .state
            .get(&key.0)
            .and_then(|c| {
                c.deliveries
                    .iter()
                    .find(|d| d.bot == key.1 && d.state == "running")
            })
            .cloned()
        else {
            return;
        };
        let Some(start) = self.channels.starts.get(&delivery.id).copied() else {
            return;
        };
        let rows = &view.bot_rows(cx)[start.min(view.bot_rows(cx).len())..];
        let (completed, status) = view.external_outcome(cx);
        let failed = !completed || rows.iter().any(|r| r.role == "error");
        let reply = bot::peers::final_reply(rows);
        let failed = failed || reply.is_err();
        let error = reply.as_ref().err().cloned().unwrap_or(status);
        let text = reply.unwrap_or_default();
        let mut next = self.channels.state.clone();
        if let Some(d) = next
            .get_mut(&key.0)
            .and_then(|c| c.deliveries.iter_mut().find(|d| d.id == delivery.id))
        {
            d.state = if failed { "interrupted" } else { "done" }.into();
            d.error = failed.then(|| format!("{error}\nInspect its conversation before retrying"));
        }
        let silent = matches!(
            text.trim().to_ascii_lowercase().as_str(),
            "" | "pass" | "<pass/>" | "<pass></pass>"
        );
        if !failed && !silent {
            let text = text.trim().trim_end_matches("<pass/>").trim();
            if let Err(e) = next.post(
                &key.0,
                self.roster.get(&key.1),
                text,
                &format!("reply:{}", delivery.id),
                delivery.hop,
                &[],
                &self.roster,
            ) {
                if let Some(d) = next
                    .get_mut(&key.0)
                    .and_then(|c| c.deliveries.iter_mut().find(|d| d.id == delivery.id))
                {
                    d.state = "interrupted".into();
                    d.error = Some(e.clone());
                }
                self.error = Some(e);
            }
        }
        match self.commit_channels(next, cx) {
            Ok(()) => {
                self.channels.starts.remove(&delivery.id);
            }
            Err(e) => self.error = Some(e),
        }
        self.channel_tick(cx);
    }
    pub(super) fn channels_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.channel_surface(window, cx)
    }
}

#[cfg(test)]
mod classic_rail_tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};
    use std::sync::{Arc, Mutex};
    use vorkspace_core::{bot::channels::Channel, store::Store};

    #[gpui::test]
    fn rail_opens_channels_without_a_selected_bot_and_returns_to_the_workspace(
        cx: &mut TestAppContext,
    ) {
        let root = std::env::temp_dir().join(format!("classic-channel-{}", bot::new_id()));
        std::fs::create_dir_all(&root).unwrap();
        let store = Arc::new(Mutex::new(
            Store::open(&root.join("native.sqlite")).unwrap(),
        ));
        let project = root.to_string_lossy().into_owned();
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| Shell::new(store, Vec::new(), project, None, cx))
            })
            .unwrap()
        });
        cx.run_until_parked();
        let manager = window
            .update(cx, |shell, _, _| shell.bot_manager())
            .unwrap();
        manager.update(cx, |manager, cx| {
            assert!(manager.active.is_none());
            manager.roster.bots.push(Bot { id: "reader".into(), name: "Reader".into(), ..Default::default() });
            manager.channels.state.channels.push(Channel {
                id: "team".into(),
                name: "team".into(),
                member_ids: vec!["reader".into()],
                messages: vec![json!({"id":"first","from":{"kind":"bot","botId":"reader"},"text":"First message"})],
                ..Default::default()
            });
            manager.changed(cx);
        });
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.simulate_resize(gpui::size(px(1400.), px(900.)));
        window
            .update(visual, |_, window, _| window.activate_window())
            .unwrap();
        visual.run_until_parked();
        let channel = visual.debug_bounds("classic-channel-team").unwrap();
        let label = visual.debug_bounds("channel-label-team").unwrap();
        let bot = visual.debug_bounds("bot-rail-reader").unwrap();
        let project = visual.debug_bounds("classic-project-label-0").unwrap();
        assert_eq!(label.left(), bot.left() + px(8.));
        assert_eq!(label.left(), project.left());
        assert!(visual.debug_bounds("channel-unread-team").is_some());
        visual.simulate_click(channel.center(), Default::default());
        visual.run_until_parked();
        manager.read_with(visual, |manager, _| {
            assert!(manager.classic_channels_open());
            assert!(manager.active.is_none());
            assert_eq!(manager.channels.selected.as_deref(), Some("team"));
            assert!(!manager.channel_unread("team"));
        });
        assert!(visual.debug_bounds("channel-unread-team").is_none());
        manager.update(visual, |manager, cx| {
            let mut next = manager.channels.state.clone();
            next.get_mut("team").unwrap().messages.push(json!({"id":"visible","from":{"kind":"bot","botId":"reader"},"text":"Visible reply"}));
            manager.commit_channels(next, cx).unwrap();
            assert!(!manager.channel_unread("team"));
        });
        visual.deactivate_window();
        manager.update(visual, |manager, cx| {
            let mut next = manager.channels.state.clone();
            next.get_mut("team").unwrap().messages.push(json!({"id":"background","from":{"kind":"bot","botId":"reader"},"text":"Reply while inactive"}));
            manager.commit_channels(next, cx).unwrap();
            assert!(manager.channel_unread("team"));
            let db = locked(&manager.store).unwrap();
            let seen: HashMap<String, String> = serde_json::from_str(&db.setting(READ_KEY).unwrap().unwrap()).unwrap();
            let restored = Channels::load(&db).unwrap();
            assert_eq!(seen["team"], "visible");
            assert_eq!(restored.get("team").unwrap().messages.last().unwrap()["id"], "background");
        });
        visual.run_until_parked();
        manager.read_with(visual, |manager, cx| {
            assert!(cx.active_window().is_none());
            assert!(manager.channel_unread("team"));
        });
        assert!(visual.debug_bounds("channel-unread-team").is_some());
        window
            .update(visual, |_, window, _| window.activate_window())
            .unwrap();
        visual.refresh().unwrap();
        visual.run_until_parked();
        assert!(visual.debug_bounds("channel-unread-team").is_none());
        assert!(visual.debug_bounds("channel-messages").is_some());
        assert!(visual.debug_bounds("classic-project-rail").is_some());
        assert!(visual.debug_bounds("sidebar-frame").is_none());

        assert!(visual.debug_bounds("close-channels").is_none());
        let close = visual.debug_bounds("classic-project-0").unwrap();
        visual.simulate_click(close.center(), Default::default());
        visual.run_until_parked();
        assert!(!manager.read_with(visual, |manager, _| manager.classic_channels_open()));
        assert!(visual.debug_bounds("sidebar-frame").is_some());

        let add = visual.debug_bounds("classic-add-channel").unwrap();
        visual.simulate_click(add.center(), Default::default());
        visual.run_until_parked();
        manager.read_with(visual, |manager, _| {
            assert!(manager.classic_channels_open());
            assert!(manager.channels.selected.is_none());
        });
        visual.simulate_input("focused-channel-name");
        visual.run_until_parked();
        manager.read_with(visual, |manager, cx| {
            assert_eq!(
                manager.fields["channel-new:name"].read(cx).content,
                "focused-channel-name"
            );
        });
        let project = visual.debug_bounds("classic-project-0").unwrap();
        visual.simulate_click(project.center(), Default::default());
        visual.run_until_parked();
        assert!(!manager.read_with(visual, |manager, _| manager.classic_channels_open()));
        assert!(visual.debug_bounds("sidebar-frame").is_some());

        // Right-clicking a channel in the sidebar opens its appearance options menu.
        let channel = visual.debug_bounds("classic-channel-team").unwrap();
        let at = channel.center();
        visual.simulate_event(gpui::MouseDownEvent {
            position: at,
            button: MouseButton::Right,
            modifiers: Default::default(),
            click_count: 1,
            first_mouse: false,
        });
        visual.simulate_event(gpui::MouseUpEvent {
            position: at,
            button: MouseButton::Right,
            modifiers: Default::default(),
            click_count: 1,
        });
        visual.run_until_parked();
        let grid_slider = visual
            .debug_bounds("channel-grid-slider")
            .expect("right-click opens the channel appearance menu");
        let click_x = grid_slider.left() + grid_slider.size.width * (4. / 6.);
        visual.simulate_click(
            gpui::point(click_x, grid_slider.center().y),
            Default::default(),
        );
        visual.run_until_parked();
        let color_btn = visual.debug_bounds("channel-color-4").unwrap();
        visual.simulate_click(color_btn.center(), Default::default());
        visual.run_until_parked();
        let tex_toggle = visual.debug_bounds("channel-texture-toggle").unwrap();
        visual.simulate_click(tex_toggle.center(), Default::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("channel-grid-slider").is_none());
        manager.read_with(visual, |manager, _| {
            let ch = manager.channels.state.get("team").unwrap();
            assert_eq!(ch.grid, Some(6));
            assert_eq!(ch.color, Some(COLORS[4]));
            assert!(!ch.textured);
        });
    }
}
