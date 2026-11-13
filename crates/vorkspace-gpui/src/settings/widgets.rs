//! Shared layout and controls for the native Settings pages.
use super::{SettingsView, SliderKind, SliderTrack};
pub(crate) use crate::controls::{primary_button, secondary_button};
use crate::input;
use crate::popover::{Align, Anchor, AnchorSlot, Side, popover};
use gpui::{
    AnyElement, Context, Div, ElementId, FontWeight, Hsla, MouseButton, MouseDownEvent, Role,
    SharedString, Stateful, div, prelude::*, px, relative,
};
use std::rc::Rc;
use vorkspace_ui::radius;
use vorkspace_ui::theme::*;

/// `text-[12px] leading-relaxed` and `text-[13px]` line boxes.
const RELAXED_12: f32 = 19.5;
const SELECT_ROW_HEIGHT: f32 = 28.;

pub(super) use crate::controls::red;
pub(super) fn accent(alpha: f32) -> Hsla {
    let mut color: Hsla = accent_color();
    color.a = alpha;
    color
}

/// PageHeader: the page's title and what it is for.
pub(super) fn page_header(title: &'static str, description: &'static str) -> Div {
    div()
        .debug_selector(|| "settings-page-header".into())
        .pb_4()
        .child(
            div()
                .text_size(px(20.))
                .line_height(px(25.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(content(1.))
                .child(title),
        )
        .child(
            div()
                .mt(px(6.))
                .max_w(px(576.))
                .text_size(px(13.))
                .line_height(px(21.125))
                .text_color(content(0.45))
                .child(description),
        )
}

/// A muted line inside a group card: an empty state or a hint.
pub(super) fn note(text: impl Into<SharedString>) -> AnyElement {
    div()
        .px_4()
        .py(px(14.))
        .text_size(px(12.))
        .line_height(px(16.))
        .text_color(content(0.45))
        .child(text.into())
        .into_any_element()
}

/// An empty list in a settings section: the shared empty-state mark, a
/// title and what to do next, in a bordered card where the list would be.
pub(super) fn empty_card(
    glyph: &'static str,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
) -> Div {
    empty_inline(glyph, title, detail)
        .rounded(px(vorkspace_ui::radius::SURFACE))
        .border_1()
        .border_color(content(0.1))
}

/// The empty state for a list that already sits inside a group's card.
pub(super) fn empty_inline(
    glyph: &'static str,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
) -> Div {
    let title = title.into();
    let detail = detail.into();
    div()
        .px_4()
        .py_6()
        .flex()
        .flex_col()
        .items_center()
        .gap_1()
        .text_center()
        .child(
            vorkspace_ui::fluid::empty_mark(icon(glyph, 20.).text_color(content(0.55)), 40.).mb_2(),
        )
        .child(
            div()
                .id(title.clone())
                .role(Role::Status)
                .aria_label(title.clone())
                .aria_description(detail.clone())
                .text_size(px(13.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(title),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(content(0.5))
                .child(detail),
        )
}

/// A muted line on the page itself, aligned with section text rather than
/// inset like `note` inside a card.
pub(super) fn page_note(text: impl Into<SharedString>) -> AnyElement {
    div()
        .min_w_0()
        .flex_1()
        .text_size(px(12.))
        .line_height(px(18.))
        .text_color(content(0.45))
        .child(text.into())
        .into_any_element()
}

/// A page-level hint with the gap that separates it from what is above.
pub(super) fn page_hint(text: impl Into<SharedString>) -> AnyElement {
    div().mt_3().child(page_note(text)).into_any_element()
}

/// Borderless, square icon actions used by SkillsPage and BotRow.
pub(super) fn icon_action(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    path: &'static str,
    side: f32,
    icon_side: f32,
    alpha: f32,
    disabled: bool,
) -> Stateful<Div> {
    let label = label.into();
    crate::controls::button(id, label.clone(), !disabled)
        .tooltip(move |_, cx| cx.new(|_| crate::controls::Tooltip(label.clone())).into())
        .group("settings-icon-action")
        .size(px(side))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(if side == 20. {
            radius::SMALL
        } else {
            radius::CONTROL
        }))
        .text_color(content(alpha))
        .when(disabled, |d| d.opacity(0.4))
        .when(!disabled, |d| {
            d.cursor_pointer().hover(|s| {
                s.bg(vorkspace_ui::fluid::hover(0.1))
                    .text_color(content(1.))
            })
        })
        .child(
            icon(path, icon_side)
                .text_color(content(alpha))
                .when(!disabled, |i| {
                    i.group_hover("settings-icon-action", |s| s.text_color(content(1.)))
                }),
        )
}

struct SelectTrigger<'a> {
    id: &'static str,
    label: &'static str,
    open: bool,
    selected: Option<usize>,
    slot: &'a AnchorSlot,
    trigger_label: SharedString,
    trigger_icon: Option<AnyElement>,
}
struct SelectKeys {
    values: Vec<String>,
    count: usize,
}

impl SettingsView {
    pub(super) fn flashing(&self, id: Option<&str>) -> bool {
        id.is_some() && self.revealed.as_deref() == id
    }

    /// A settings card without a redundant section heading on single-group pages.
    pub(super) fn card(&self, id: Option<&'static str>, children: Vec<AnyElement>) -> Div {
        div()
            .relative()
            .when_some(id, |d, id| d.child(self.reveal_probe(id)))
            .overflow_hidden()
            .rounded(px(radius::SURFACE))
            .border_1()
            .border_color(if self.flashing(id) {
                accent(0.6)
            } else {
                content(0.1)
            })
            .bg(content(0.03))
            .flex()
            .flex_col()
            .children(children)
    }

    /// Group: a titled card. `first` drops the `pt-8` the CSS `first:` does.
    pub(super) fn group(
        &self,
        id: Option<&'static str>,
        first: bool,
        title: impl IntoElement,
        description: Option<SharedString>,
        action: Option<AnyElement>,
        children: Vec<AnyElement>,
    ) -> AnyElement {
        div()
            .relative()
            .when(!first, |d| d.pt_8())
            .when_some(id, |d, id| d.child(self.reveal_probe(id)))
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap_4()
                    .pb(px(10.))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .line_height(px(19.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(content(1.))
                                    .child(title),
                            )
                            .when_some(description, |d, text| {
                                d.child(
                                    div()
                                        .mt_1()
                                        .text_size(px(12.))
                                        .line_height(px(RELAXED_12))
                                        .text_color(content(0.45))
                                        .child(text),
                                )
                            }),
                    )
                    .when_some(action, |d, action| {
                        d.child(div().flex_shrink_0().pb(px(2.)).child(action))
                    }),
            )
            .child(
                self.card(None, children)
                    .when(self.flashing(id), |d| d.border_color(accent(0.6))),
            )
            .into_any_element()
    }

    /// Row: a label and description beside its controls. Rows draw their
    /// own top rule, which reads the same as Tailwind's `border-b` with
    /// `last:border-b-0`.
    pub(super) fn row(
        &self,
        id: Option<&'static str>,
        first: bool,
        label: impl IntoElement,
        description: Option<SharedString>,
        controls: Vec<AnyElement>,
    ) -> AnyElement {
        let flash = self.flashing(id);
        let narrow =
            self.scroll.bounds().size.width > px(0.) && self.scroll.bounds().size.width < px(560.);
        let toggle_only = Self::row_toggle_only(id);
        let stacked = narrow && !toggle_only;
        let compact = description.is_none() && !stacked;
        div()
            .debug_selector(|| format!("settings-row-{}", id.unwrap_or("anonymous")))
            .relative()
            .flex()
            .map(|d| {
                if compact {
                    d.items_center()
                } else {
                    d.items_start()
                }
            })
            .map(|d| {
                if stacked {
                    d.flex_col().gap_3()
                } else {
                    d.gap_6()
                }
            })
            .px_4()
            .py(px(14.))
            .when(!first, |d| d.border_t_1().border_color(content(0.05)))
            .when(flash, |d| d.bg(accent(0.1)))
            .when_some(id, |d, id| d.child(self.reveal_probe(id)))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .child(
                        div()
                            .text_size(px(13.))
                            .line_height(px(19.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(content(1.))
                            .child(label),
                    )
                    .when_some(description, |d, text| {
                        d.child(
                            div()
                                .mt_1()
                                .text_size(px(12.))
                                .line_height(px(RELAXED_12))
                                .text_color(content(0.45))
                                .child(text),
                        )
                    }),
            )
            .child(
                div()
                    .min_w_0()
                    .map(|d| {
                        if stacked {
                            d.max_w_full()
                        } else {
                            d.max_w(relative(0.6))
                        }
                    })
                    .flex_shrink_0()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .map(|d| {
                        if stacked {
                            d.justify_start()
                        } else {
                            d.justify_end()
                        }
                    })
                    .gap_2()
                    .children(controls),
            )
            .into_any_element()
    }

    fn row_toggle_only(id: Option<&'static str>) -> bool {
        id.is_some_and(|id| {
            matches!(
                id,
                "sounds"
                    | "notifications"
                    | "tab-animations"
                    | "notes"
                    | "todos"
                    | "vox-enabled"
                    | "vox-silent"
                    | "bot-minions"
                    | "quick-composer"
                    | "clipboard-history"
                    | "dictation-enabled"
                    | "working-agents"
                    | "close-to-tray"
                    | "main-pane-glass"
                    | "show-excluded-files"
                    | "anchor-prompts"
                    | "paced-reveal"
                    | "composer-autocorrect"
                    | "format-on-save"
                    | "show-remaining-usage"
                    | "mask-emails"
                    | "claude-hooks"
                    | "bots-enabled"
                    | "bot-menu-bar-icon"
                    | "show-archived"
            )
        })
    }

    /// Where search lands: each target reports its place in the page.
    pub(in crate::settings) fn reveal_probe(&self, id: &'static str) -> AnyElement {
        let slot = self.targets.borrow_mut().entry(id).or_default().clone();
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(slot.layout_probe())
            .into_any_element()
    }

    /// Toggle: a 36×20 switch with a 16px knob.
    pub(super) fn toggle(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        on: bool,
        disabled: bool,
        cx: &mut Context<Self>,
        change: impl Fn(&mut Self, bool, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        crate::controls::switch(
            SharedString::from(format!("toggle-{id}")),
            label,
            on,
            !disabled,
        )
        .debug_selector(move || format!("settings-toggle-{id}"))
        .when(disabled, |d| d.opacity(0.4))
        .when(!disabled, |d| {
            d.cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| change(this, !on, cx)))
        })
        .into_any_element()
    }

    /// Segmented: equal-width options in a hairline frame.
    pub(super) fn segmented(
        &self,
        id: &'static str,
        label: &'static str,
        value: &str,
        options: &[(&'static str, &str)],
        cx: &mut Context<Self>,
        change: impl Fn(&mut Self, &'static str, &mut Context<Self>) + Clone + 'static,
    ) -> AnyElement {
        let selected = options.iter().position(|&(option, _)| value == option);
        let options: Vec<(&'static str, String)> = options
            .iter()
            .map(|&(option, text)| (option, text.to_owned()))
            .collect();
        let columns = options.len() as u16;
        let entity = cx.entity().downgrade();
        div()
            .id(SharedString::from(format!("segmented-{id}")))
            .role(Role::RadioGroup)
            .aria_label(label)
            .flex_shrink_0()
            // The selection is one thumb that slides between the options.
            .child(vorkspace_ui::glide::GlideList::new(
                SharedString::from(format!("segmented-glide-{id}")),
                selected,
                div()
                    .grid()
                    .grid_cols(columns)
                    .gap(px(2.))
                    .rounded(px(radius::CONTROL))
                    .border_1()
                    .border_color(content(0.1))
                    .p(px(2.))
                    .text_size(px(12.))
                    .line_height(px(16.)),
                content(0.1),
                radius::CONTROL - 2. - 1.,
                move |glide| {
                    let ready = glide.ready();
                    options
                        .into_iter()
                        .enumerate()
                        .map(|(index, (option, text))| {
                            let selected = selected == Some(index);
                            let change = change.clone();
                            let entity = entity.clone();
                            div()
                                .id(SharedString::from(format!("segmented-{id}-{option}")))
                                .debug_selector(move || format!("settings-{id}-{option}"))
                                .role(Role::RadioButton)
                                .aria_label(text.clone())
                                .aria_selected(selected)
                                .map(vorkspace_ui::theme::tab_reachable)
                                .relative()
                                .min_w_0()
                                .flex()
                                .justify_center()
                                .whitespace_nowrap()
                                .rounded(px(radius::CONTROL - 2. - 1.))
                                .px(px(10.))
                                .py_1()
                                .cursor_pointer()
                                .child(glide.probe(index))
                                .map(|d| {
                                    if selected {
                                        d.when(!ready, |d| d.bg(content(0.1)))
                                            .text_color(content(1.))
                                    } else {
                                        d.text_color(content(0.5))
                                            .hover(|s| s.text_color(content(1.)))
                                    }
                                })
                                .child(text)
                                .on_click(move |_, _, cx| {
                                    entity.update(cx, |this, cx| change(this, option, cx)).ok();
                                })
                                .into_any_element()
                        })
                        .collect()
                },
            ))
            .into_any_element()
    }

    /// Slider: a 4px track and 12px thumb, then its value, in a 224px row.
    pub(super) fn slider(
        &self,
        kind: SliderKind,
        display: String,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (min, max) = kind.range();
        let value = kind.get(&self.prefs);
        let fraction = if max > min {
            (value.saturating_sub(min)) as f32 / (max - min) as f32
        } else {
            0.
        };
        let track = self
            .slider_tracks
            .borrow_mut()
            .entry(kind)
            .or_insert_with(|| SliderTrack {
                bounds: AnchorSlot::default(),
                focus: cx.focus_handle(),
            })
            .clone();
        div()
            .w(px(224.))
            .max_w_full()
            .flex()
            .items_center()
            .gap_3()
            .when(disabled, |d| d.opacity(0.4))
            .child(
                div()
                    .id(SharedString::from(format!("slider-{}", kind.id())))
                    .debug_selector(move || format!("settings-slider-{}", kind.id()))
                    .role(Role::Slider)
                    .aria_label(kind.label())
                    .aria_disabled(disabled)
                    .aria_value(display.clone())
                    .aria_numeric_value(f64::from(value))
                    .aria_min_numeric_value(f64::from(min))
                    .aria_max_numeric_value(f64::from(max))
                    .aria_numeric_value_step(1.)
                    .min_w_0()
                    .flex_1()
                    .h(px(12.))
                    .relative()
                    .flex()
                    .items_center()
                    .child(track.bounds.layout_probe())
                    .child(
                        div()
                            .w_full()
                            .h(px(4.))
                            .rounded_full()
                            .bg(vorkspace_ui::fluid::track(0.15)),
                    )
                    // fluidither: solid ink where the value is, dots ahead of it.
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top(px(4.))
                            .h(px(4.))
                            .w(relative(fraction))
                            .rounded_full()
                            .bg(content(0.85)),
                    )
                    .child(
                        vorkspace_ui::fluid::thumb(content(1.))
                            .top(px(-2.))
                            .left(relative(fraction))
                            .ml(px(-4. + 8. * (0.5 - fraction))),
                    )
                    .when(!disabled, |d| {
                        d.map(tab_reachable)
                            .track_focus(&track.focus)
                            .cursor_pointer()
                            .on_key_down(cx.listener(
                                move |this, event: &gpui::KeyDownEvent, _, cx| {
                                    this.slider_key(kind, min, max, event, cx);
                                },
                            ))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                    window.focus(&track.focus, cx);
                                    this.slider_drag = Some(kind);
                                    this.slide(kind, event.position.x, cx);
                                    cx.stop_propagation();
                                }),
                            )
                    }),
            )
            .child(Self::slider_value(display))
            .into_any_element()
    }

    fn slider_value(display: String) -> Div {
        div()
            .w(px(40.))
            .flex_shrink_0()
            .text_right()
            .text_size(px(12.))
            .line_height(px(16.))
            .text_color(content(1.))
            .child(display)
    }

    fn slider_key(
        &mut self,
        kind: SliderKind,
        min: u32,
        max: u32,
        event: &gpui::KeyDownEvent,
        cx: &mut Context<Self>,
    ) {
        let modifiers = event.keystroke.modifiers;
        if modifiers.platform || modifiers.control || modifiers.alt || modifiers.shift {
            return;
        }
        let current = kind.get(&self.prefs).clamp(min, max);
        let value = match event.keystroke.key.as_str() {
            "left" | "down" => current.saturating_sub(1).max(min),
            "right" | "up" => current.saturating_add(1).min(max),
            "home" => min,
            "end" => max,
            _ => return,
        };
        if value != kind.get(&self.prefs) {
            self.update(cx, |p| kind.set(p, value));
        }
        cx.stop_propagation();
    }
    pub(super) fn slide(&mut self, kind: SliderKind, x: gpui::Pixels, cx: &mut Context<Self>) {
        let Some(bounds) = self
            .slider_tracks
            .borrow()
            .get(&kind)
            .and_then(|track| track.bounds.get())
        else {
            return;
        };
        let (min, max) = kind.range();
        // A range input's thumb travels the track less its own width.
        let travel = (bounds.size.width.as_f32() - 12.).max(1.);
        let fraction = ((x - bounds.left()).as_f32() - 6.).clamp(0., travel) / travel;
        let value = min + ((max - min) as f32 * fraction).round() as u32;
        if kind.get(&self.prefs) != value {
            self.update(cx, |p| kind.set(p, value));
        }
    }

    /// Select: a bordered trigger opening a listbox popover below it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn select(
        &self,
        id: &'static str,
        label: &'static str,
        value: &str,
        options: Vec<(String, SharedString, Option<AnyElement>)>,
        width: Option<f32>,
        cx: &mut Context<Self>,
        change: impl Fn(&mut Self, String, &mut Context<Self>) + Clone + 'static,
    ) -> AnyElement {
        let open = self.select_open == Some(id);
        let slot: AnchorSlot = self
            .select_anchors
            .borrow_mut()
            .entry(id)
            .or_default()
            .clone();
        let selected = options.iter().position(|(v, _, _)| v == value);
        let mut options = options;
        let (trigger_label, trigger_icon) = match selected {
            Some(ix) => {
                let (_, text, icon) = &mut options[ix];
                (text.clone(), icon.take())
            }
            None => (SharedString::from(value.to_owned()), None),
        };
        let values: Vec<String> = options.iter().map(|(v, _, _)| v.clone()).collect();
        let weak = cx.weak_entity();
        div()
            .relative()
            .map(|d| match width {
                Some(w) => d.w(px(w)),
                None => d.max_w(px(208.)),
            })
            .child(self.select_trigger(
                SelectTrigger {
                    id,
                    label,
                    open,
                    selected,
                    slot: &slot,
                    trigger_label,
                    trigger_icon,
                },
                cx,
            ))
            .when(open, |d| {
                let active = self.select_active;
                let rows = Self::select_rows(id, selected, active, options, &change, cx);
                let count = rows.len();
                let keys_change = change.clone();
                let keys = SelectKeys { values, count };
                d.child(
                    popover(
                        SharedString::from(format!("select-popover-{id}")),
                        Anchor::Slot(slot),
                    )
                    .side(Side::Bottom)
                    .align(Align::End)
                    .width(280.)
                    .max_height(320.)
                    .padding(4.)
                    .on_dismiss(move |window, cx| {
                        let _ = weak.update(cx, |this, cx| {
                            this.select_open = None;
                            window.focus(&this.focus, cx);
                            cx.notify();
                        });
                    })
                    .child(
                        div()
                            .id(SharedString::from(format!("select-list-{id}")))
                            .debug_selector(move || format!("settings-select-scroll-{id}"))
                            .role(Role::ListBox)
                            .aria_label(label)
                            .track_focus(&self.select_focus)
                            .track_scroll(&self.select_scroll)
                            .max_h(px(310.))
                            .min_h_0()
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .on_key_down(cx.listener(
                                move |this, event: &gpui::KeyDownEvent, window, cx| {
                                    this.select_key(event, window, cx, &keys, &keys_change);
                                },
                            ))
                            .children(rows),
                    ),
                )
            })
            .into_any_element()
    }

    fn select_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
        keys: &SelectKeys,
        keys_change: &impl Fn(&mut Self, String, &mut Context<Self>),
    ) {
        let SelectKeys { values, count } = keys;

        let key = event.keystroke.key.as_str();
        let last = count.saturating_sub(1);
        match key {
            "down" => self.select_active = (self.select_active + 1).min(last),
            "up" => self.select_active = self.select_active.saturating_sub(1),
            "home" => self.select_active = 0,
            "end" => self.select_active = last,
            "enter" | "tab" => {
                if let Some(v) = values.get(self.select_active) {
                    keys_change(self, v.clone(), cx);
                }
                self.select_open = None;
                window.focus(&self.focus, cx);
            }
            "escape" => {
                self.select_open = None;
                window.focus(&self.focus, cx);
            }
            _ => return,
        }
        if matches!(key, "down" | "up" | "home" | "end") {
            let top = px(self.select_active as f32 * SELECT_ROW_HEIGHT);
            let height = self
                .select_scroll
                .bounds()
                .size
                .height
                .max(px(SELECT_ROW_HEIGHT));
            let offset = self.select_scroll.offset();
            let y = offset.y.clamp(-top, height - top - px(SELECT_ROW_HEIGHT));
            self.select_scroll.set_offset(gpui::point(offset.x, y));
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn select_rows<C: Fn(&mut Self, String, &mut Context<Self>) + Clone + 'static>(
        id: &'static str,
        selected: Option<usize>,
        active: usize,
        options: Vec<(String, SharedString, Option<AnyElement>)>,
        change: &C,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        options
            .into_iter()
            .enumerate()
            .map(|(ix, (option, text, icon_el))| {
                let is_selected = Some(ix) == selected;
                let highlighted = ix == active;
                let change = change.clone();
                div()
                    .id(SharedString::from(format!("select-{id}-{ix}")))
                    .debug_selector(move || format!("settings-option-{id}-{ix}"))
                    .role(Role::ListBoxOption)
                    .aria_label(text.clone())
                    .aria_selected(is_selected)
                    .w_full()
                    .h(px(SELECT_ROW_HEIGHT))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(radius::CONTAINER))
                    .px_2()
                    .py(px(6.))
                    .text_size(px(12.))
                    .line_height(px(16.))
                    .text_color(content(1.))
                    .cursor_pointer()
                    .map(|d| {
                        if highlighted || is_selected {
                            d.bg(content(0.1))
                        } else {
                            d.hover(|s| s.bg(vorkspace_ui::fluid::hover(0.05)))
                        }
                    })
                    .when_some(icon_el, |d, icon_el| {
                        d.child(
                            div()
                                .size_4()
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(icon_el),
                        )
                    })
                    .child(div().min_w_0().flex_1().truncate().child(text))
                    .when(is_selected, |d| {
                        d.child(icon("check-hi", 14.).text_color(content(1.)))
                    })
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if this.select_active != ix {
                            this.select_active = ix;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_open = None;
                        change(this, option.clone(), cx);
                        window.focus(&this.focus, cx);
                        cx.notify();
                    }))
            })
            .collect::<Vec<_>>()
    }

    fn select_trigger(&self, trigger: SelectTrigger<'_>, cx: &mut Context<Self>) -> Stateful<Div> {
        let SelectTrigger {
            id,
            label,
            open,
            selected,
            slot,
            trigger_label,
            trigger_icon,
        } = trigger;
        div()
            .id(SharedString::from(format!("select-{id}")))
            .debug_selector(move || format!("settings-select-{id}"))
            .role(Role::ComboBox)
            .aria_label(format!("{label}: {trigger_label}"))
            .aria_expanded(open)
            .relative()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .rounded(px(radius::CONTROL))
            .border_1()
            .border_color(content(0.1))
            .bg(content(0.05))
            .px_2()
            .py_1()
            .text_size(px(12.))
            .line_height(px(16.))
            .text_color(content(1.))
            .cursor_pointer()
            .hover(|s| s.border_color(content(0.2)))
            .child(slot.probe())
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .when_some(trigger_icon, |d, icon| {
                        d.child(
                            div()
                                .size_4()
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(icon),
                        )
                    })
                    .child(div().min_w_0().truncate().child(trigger_label)),
            )
            .child(
                icon("chevron-down-hi", 14.)
                    .text_color(content(0.5))
                    .when(open, |s| {
                        s.with_transformation(gpui::Transformation::rotate(gpui::percentage(0.5)))
                    }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.select_open == Some(id) {
                    this.select_open = None;
                } else {
                    this.select_open = Some(id);
                    this.select_active = selected.unwrap_or(0);
                    // Use list-relative row positions: a reopened popover
                    // can be placed somewhere else or still be animating.
                    this.select_scroll.set_offset(gpui::point(
                        px(0.),
                        px(-(this.select_active as f32) * SELECT_ROW_HEIGHT),
                    ));
                    window.focus(&this.select_focus, cx);
                }
                cx.notify();
            }))
    }
}

impl SettingsView {
    fn toggle_model_visibility(&mut self, provider: &str, id: &str, cx: &mut Context<Self>) {
        let key = vorkspace_ui::preferences::model_key(provider, id);
        self.update(cx, |p| {
            if let Some(at) = p.hidden_models.iter().position(|saved| *saved == key) {
                p.hidden_models.remove(at);
            } else {
                p.hidden_models.push(key);
            }
        });
    }

    fn step_model_visibility(&mut self, ids: &[String], step: isize, cx: &mut Context<Self>) {
        if ids.is_empty() || self.model_filter.read(cx).is_composing() {
            return;
        }
        let last = ids.len() - 1;
        let active = ids
            .iter()
            .position(|id| self.model_active.as_ref() == Some(id));
        let next = active.map_or_else(
            || if step < 0 { last } else { 0 },
            |at| at.saturating_add_signed(step).min(last),
        );
        self.model_active = Some(ids[next].clone());
        self.model_scroll.scroll_to_item(next);
        cx.stop_propagation();
        cx.notify();
    }

    /// Choose which of a provider's models the picker offers. The full catalog
    /// stays listed here so a hidden model can always be restored.
    pub(super) fn models_menu(
        &self,
        kind: vorkspace_core::provider::Kind,
        models: &[(String, String)],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let provider = kind.id();
        let open = self.models_open == Some(kind);
        let slot = self
            .model_menu_anchors
            .borrow_mut()
            .entry(kind)
            .or_default()
            .clone();
        let visible = models
            .iter()
            .filter(|(id, _)| !self.prefs.model_hidden(provider, id))
            .count();
        let weak = cx.weak_entity();
        let trigger = Self::models_trigger(kind, visible, models, open, &slot, cx);
        div()
            .relative()
            .child(trigger)
            .when(open, |d| {
                let query = self.model_filter.read(cx).content.trim().to_lowercase();
                let filtered = models
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, name))| {
                        query.is_empty() || name.to_lowercase().contains(&query)
                    })
                    .collect::<Vec<_>>();
                let ids = filtered
                    .iter()
                    .map(|(_, (id, _))| id.clone())
                    .collect::<Rc<[_]>>();
                let up_ids = ids.clone();
                let down_ids = ids.clone();
                let rows = filtered
                    .into_iter()
                    .map(|(ix, (id, name))| self.model_visibility_row(provider, ix, id, name, cx))
                    .collect::<Vec<_>>();
                let any_hidden = visible < models.len();
                d.child(
                    popover(
                        SharedString::from(format!("models-popover-{provider}")),
                        Anchor::Slot(slot),
                    )
                    .side(Side::Bottom)
                    .align(Align::End)
                    .width(320.)
                    .max_height(380.)
                    .padding(4.)
                    .on_dismiss(move |window, cx| {
                        let _ = weak.update(cx, |this, cx| {
                            this.models_open = None;
                            window.focus(&this.focus, cx);
                            cx.notify();
                        });
                    })
                    .child(
                        div()
                            .id(SharedString::from(format!("models-list-{provider}")))
                            .role(Role::Group)
                            .aria_label(format!("{} model visibility", kind.label()))
                            .capture_action(cx.listener(move |this, _: &input::Up, _, cx| {
                                this.step_model_visibility(&up_ids, -1, cx);
                            }))
                            .capture_action(cx.listener(move |this, _: &input::Down, _, cx| {
                                this.step_model_visibility(&down_ids, 1, cx);
                            }))
                            .capture_action(cx.listener(move |this, _: &input::Submit, _, cx| {
                                this.submit_model_visibility(provider, &ids, cx);
                            }))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(self.models_filter_bar(provider, any_hidden, cx))
                            .child(
                                div()
                                    .id(SharedString::from(format!("models-scroll-{provider}")))
                                    .debug_selector(move || format!("models-scroll-{provider}"))
                                    .track_scroll(&self.model_scroll)
                                    .max_h(px(320.))
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .flex()
                                    .flex_col()
                                    .children(rows),
                            ),
                    ),
                )
            })
            .into_any_element()
    }

    fn submit_model_visibility(
        &mut self,
        provider: &'static str,
        ids: &Rc<[String]>,
        cx: &mut Context<Self>,
    ) {
        if self.model_filter.read(cx).is_composing() {
            return;
        }
        let selected = ids
            .iter()
            .find(|id| self.model_active.as_ref() == Some(*id))
            .or_else(|| (ids.len() == 1).then(|| &ids[0]));
        if let Some(id) = selected {
            self.toggle_model_visibility(provider, id, cx);
            cx.stop_propagation();
        }
    }

    fn models_trigger(
        kind: vorkspace_core::provider::Kind,
        visible: usize,
        models: &[(String, String)],
        open: bool,
        slot: &AnchorSlot,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let provider = kind.id();
        secondary_button(
            SharedString::from(format!("models-{provider}")),
            format!("Choose visible {} models", kind.label()),
            false,
            models.is_empty(),
        )
        .relative()
        .gap_1()
        .debug_selector(move || format!("models-{provider}"))
        .child(slot.probe())
        .child(format!("Models {visible}/{}", models.len()))
        .when(!models.is_empty(), |d| {
            d.aria_expanded(open)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.toggle_models_menu(kind, window, cx);
                }))
        })
    }

    fn toggle_models_menu(
        &mut self,
        kind: vorkspace_core::provider::Kind,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        if self.models_open == Some(kind) {
            self.models_open = None;
        } else {
            self.models_open = Some(kind);
            self.model_active = None;
            self.model_scroll.set_offset(gpui::point(px(0.), px(0.)));
            self.model_filter
                .update(cx, |input, cx| input.set_text(String::new(), cx));
            window.focus(&gpui::Focusable::focus_handle(&self.model_filter, cx), cx);
        }
        cx.notify();
    }

    fn models_filter_bar(
        &self,
        provider: &'static str,
        any_hidden: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .child(div().flex_1().min_w_0().child(self.model_filter.clone()))
            .when(any_hidden, |d| {
                d.child(
                    secondary_button(
                        SharedString::from(format!("models-show-all-{provider}")),
                        "Show all models",
                        false,
                        false,
                    )
                    .child("Show all")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.update(cx, |p| {
                            let prefix = format!("{provider}:");
                            p.hidden_models.retain(|key| !key.starts_with(&prefix));
                        })
                    })),
                )
            })
    }

    fn model_visibility_row(
        &self,
        provider: &'static str,
        ix: usize,
        id: &String,
        name: &str,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let shown = !self.prefs.model_hidden(provider, id);
        let active = self.model_active.as_deref() == Some(id);
        let model = id.clone();
        div()
            .id(SharedString::from(format!("model-visible-{provider}-{id}")))
            .debug_selector(move || format!("model-visible-{provider}-{ix}"))
            .role(Role::CheckBox)
            .aria_label(name.to_owned())
            .aria_toggled(if shown {
                gpui::Toggled::True
            } else {
                gpui::Toggled::False
            })
            .when(active, |d| d.aria_active_descendant().bg(content(0.1)))
            .w_full()
            .flex()
            .items_center()
            .gap_2()
            .rounded(px(radius::CONTAINER))
            .px_2()
            .py(px(6.))
            .text_size(px(12.))
            .line_height(px(16.))
            .text_color(content(if shown { 1. } else { 0.5 }))
            .cursor_pointer()
            .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.05)))
            .child(
                div()
                    .size_4()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(shown, |d| {
                        d.child(icon("check-hi", 14.).text_color(content(1.)))
                    }),
            )
            .child(div().min_w_0().flex_1().truncate().child(name.to_owned()))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_model_visibility(provider, &model, cx);
            }))
    }
    pub(super) fn project_picker(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let options = self
            .ctx
            .projects
            .iter()
            .map(|(path, label)| {
                (
                    path.clone(),
                    SharedString::from(label.clone()),
                    Some(icon("folder", 14.).into_any_element()),
                )
            })
            .collect();
        let value = self.selected_project.clone();
        self.select(
            "settings-project",
            "Project",
            &value,
            options,
            None,
            cx,
            |this, value, cx| {
                this.selected_project = value;
                this.refresh(cx);
            },
        )
    }
    pub(super) fn refresh_button(
        &self,
        id: &'static str,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        secondary_button(id, label, false, self.discovery_loading)
            .child(icon("refresh-cw", 14.))
            .child(label)
            .when(!self.discovery_loading, |d| {
                d.on_click(cx.listener(|this, _, _, cx| this.refresh(cx)))
            })
            .into_any_element()
    }
}
