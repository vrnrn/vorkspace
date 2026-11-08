//! Per-Bot browsing stays inside its pane, independent of project toolbars.
use super::*;
use vorkspace_ui::radius;

const WIDTH: f32 = 300.;
const LANE_WIDTH: f32 = WIDTH + 16.;
// Keep the normal transcript/composer column intact while the island is docked.
pub(crate) const ISLAND_BREAKPOINT: f32 = COLUMN + LANE_WIDTH;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Section {
    Documents,
}

impl Bots {
    pub(super) fn toggle_island(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.open_islands.remove(id) {
            self.open_islands.insert(id.into());
        }
        cx.notify();
    }
    pub(crate) fn collapse_island(&mut self, id: &str) {
        self.open_islands.remove(id);
    }

    pub(super) fn island_trigger(&self, bot: &Bot, cx: &mut Context<Self>) -> AnyElement {
        let id = bot.id.clone();
        icon_button("bot-island-toggle", "Show documents", "bot-island")
            .debug_selector(|| "bot-island-toggle".into())
            .aria_expanded(false)
            .size(px(32.))
            .rounded(px(radius::CONTROL))
            .bg(island_fill())
            .border_1()
            .border_color(content(0.08))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_island(&id, cx);
                cx.stop_propagation();
            }))
            .into_any_element()
    }

    pub(super) fn island(
        &mut self,
        bot: &Bot,
        overlay: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let settings_id = bot.id.clone();
        let close_id = bot.id.clone();
        let expanded = self.expanded_islands.get(&bot.id).copied();
        let status = self
            .chats
            .get(&bot.id)
            .map(|chat| chat.read(cx).bot_status_label())
            .unwrap_or("Ready");
        // Sessions a Bot starts live in the rail's Sessions list with the
        // rest of the vorkspace, marked with the Bot that started them; the
        // island keeps what is the Bot's own: its documents.
        let mut body = div()
            .id(if expanded.is_some() {
                "bot-island-documents"
            } else {
                "bot-island-overview"
            })
            .debug_selector(|| "bot-island-content".into())
            .flex_shrink_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_2()
            .pb_2()
            .when(expanded.is_some(), |d| d.flex_1());
        if bot.documents.is_empty() {
            body = body.child(
                div()
                    .id("bot-island-fresh")
                    .debug_selector(|| "bot-island-fresh".into())
                    .px_2()
                    .pb_1()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(content(0.45))
                    .child("Documents it writes appear here."),
            );
        } else {
            body = body
                .child(section("Documents", bot.documents.len()))
                .child(self.document_list(bot, expanded.is_some(), cx));
            if expanded.is_none() && bot.documents.len() > 3 {
                body = body.child(self.island_more(bot, Section::Documents, false, cx));
            }
        }
        div()
            .id(SharedString::from(format!("bot-island-{}", bot.id)))
            .debug_selector(|| "bot-island".into())
            .role(Role::Complementary)
            .aria_label(format!("{} documents", bot.label()))
            .w_full()
            .max_h_full()
            .min_h_0()
            .when(expanded.is_some(), |d| d.h_full())
            .rounded(px(radius::MODAL))
            .border_1()
            .border_color(content(0.08))
            .bg(island_fill())
            .when(overlay, |d| d.shadow_lg())
            .overflow_hidden()
            .occlude()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_4()
                    .py_3()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .id("bot-island-status")
                            .debug_selector(|| "bot-island-status".into())
                            .role(Role::Status)
                            .flex_1()
                            .min_w_0()
                            .text_size(px(13.))
                            .text_color(content(0.75))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .truncate()
                            .child(status),
                    )
                    .child(
                        icon_button("bot-island-settings", "Bot settings", "settings")
                            .debug_selector(|| "bot-island-settings".into())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_islands.remove(&settings_id);
                                this.open_bot_settings(&settings_id, window, cx);
                                cx.stop_propagation();
                            })),
                    )
                    .when(overlay, |d| {
                        d.child(
                            icon_button("bot-island-close", "Close documents", "close")
                                .debug_selector(|| "bot-island-close".into())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_islands.remove(&close_id);
                                    cx.notify();
                                    cx.stop_propagation();
                                })),
                        )
                    }),
            )
            .child(body)
            .when_some(expanded, |d, section| {
                d.child(
                    div()
                        .px_2()
                        .pb_2()
                        .flex_shrink_0()
                        .child(self.island_more(bot, section, true, cx)),
                )
            })
            .into_any_element()
    }

    fn island_more(
        &self,
        bot: &Bot,
        section: Section,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = bot.id.clone();
        let (control, label) = match (section, expanded) {
            (Section::Documents, false) => ("bot-island-more-documents", "View all documents"),
            (_, true) => ("bot-island-show-less", "Show less"),
        };
        icon_button(
            control,
            label,
            if expanded {
                "chevron-up"
            } else {
                "chevron-down"
            },
        )
        .debug_selector(move || control.into())
        .aria_expanded(expanded)
        .w_full()
        .h(px(28.))
        .on_click(cx.listener(move |this, _, _, cx| {
            if expanded {
                this.expanded_islands.remove(&id);
            } else {
                this.expanded_islands.insert(id.clone(), section);
            }
            cx.notify();
            cx.stop_propagation();
        }))
        .into_any_element()
    }

    pub(super) fn island_lane(&mut self, bot: &Bot, cx: &mut Context<Self>) -> AnyElement {
        div()
            .w(px(LANE_WIDTH))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .py_4()
            .pr_4()
            .child(self.island(bot, false, cx))
            .into_any_element()
    }

    pub(super) fn island_overlay(
        &mut self,
        bot: &Bot,
        width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = bot.id.clone();
        div()
            .absolute()
            .inset_0()
            .child(
                div()
                    .id("bot-island-dismiss")
                    .absolute()
                    .inset_0()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.open_islands.remove(&id);
                            cx.notify();
                            cx.stop_propagation();
                        }),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .top_3()
                    .right_3()
                    .bottom_3()
                    .w(px(WIDTH.min((width - 24.).max(0.))))
                    .flex()
                    .flex_col()
                    .child(self.island(bot, true, cx)),
            )
            .into_any_element()
    }
}

fn section(title: &'static str, count: usize) -> gpui::Div {
    div()
        .px_2()
        .pt_2()
        .pb_1()
        .flex()
        .items_center()
        .gap_2()
        .text_size(px(12.))
        .text_color(content(0.5))
        .child(title)
        .when(count > 0, |d| {
            d.child(
                div()
                    .text_size(px(11.))
                    .text_color(content(0.35))
                    .child(count.to_string()),
            )
        })
}

fn island_fill() -> gpui::Hsla {
    let mut fill = floating_fill();
    if !is_light() {
        fill.l = (fill.l + 0.07).min(1.);
    }
    fill.a = (vorkspace_ui::theme::pane_fill().a + 0.2).min(1.);
    fill
}
