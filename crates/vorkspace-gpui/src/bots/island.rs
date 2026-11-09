//! Per-Bot browsing stays inside its pane, independent of project toolbars.
use super::*;
use vorkspace_ui::radius;

const WIDTH: f32 = 300.;
const LANE_WIDTH: f32 = WIDTH + 16.;
// Keep the normal transcript/composer column intact while the island is docked.
pub(crate) const ISLAND_BREAKPOINT: f32 = COLUMN + LANE_WIDTH;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Section {
    Sessions,
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
        icon_button(
            "bot-island-toggle",
            "Show launched sessions and documents",
            "bot-island",
        )
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
        let mut body = div()
            .id(match expanded {
                Some(Section::Sessions) => "bot-island-sessions",
                Some(Section::Documents) => "bot-island-documents",
                None => "bot-island-overview",
            })
            .debug_selector(|| "bot-island-content".into())
            .flex_shrink_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_2()
            .pb_2()
            .when(expanded.is_some(), |d| d.flex_1());
        if expanded != Some(Section::Documents) && !bot.launches.is_empty() {
            let limit = if expanded == Some(Section::Sessions) {
                usize::MAX
            } else {
                3
            };
            body = body
                .child(section("Launched sessions", bot.launches.len()))
                .children(bot.launches.iter().rev().take(limit).map(|launch| {
                    let session = launch.session_id;
                    let bot = bot.id.clone();
                    let title = if launch.title.is_empty() {
                        format!("Session {session}")
                    } else {
                        launch.title.clone()
                    };
                    let state = if launch.completed {
                        "Finished"
                    } else {
                        "In progress"
                    };
                    div()
                        .id(("bot-island-session", session as usize))
                        .role(Role::Button)
                        .aria_label(format!("Open session {title}"))
                        .aria_description(state)
                        .map(vorkspace_ui::theme::tab_reachable)
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_2()
                        .rounded(px(radius::CONTROL))
                        .cursor_pointer()
                        .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.08)))
                        .child(icon("message-circle", 14.).text_color(content(0.55)))
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .truncate()
                                .text_size(px(12.))
                                .child(title),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_workspace_context(Some(&bot), cx);
                            this.open_session_workspace_pane(session, cx);
                            cx.stop_propagation();
                        }))
                }));
            if expanded.is_none() && bot.launches.len() > 3 {
                body = body.child(self.island_more(bot, Section::Sessions, false, cx));
            }
        }
        if expanded == Some(Section::Sessions) {
            // The expanded view keeps its own scroll position.
        } else if bot.documents.is_empty() {
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
            .aria_label(format!("{} sessions and documents", bot.label()))
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
                            icon_button("bot-island-close", "Close Bot panel", "close")
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
            (Section::Sessions, false) => {
                ("bot-island-more-sessions", "View all launched sessions")
            }
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
