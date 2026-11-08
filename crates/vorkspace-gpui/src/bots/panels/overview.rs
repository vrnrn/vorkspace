//! Sessions and documents in the Bot island; habit navigation lives in settings.
use super::*;
use vorkspace_ui::radius;

impl Bots {
    pub(crate) fn document_list(
        &self,
        bot: &Bot,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut documents: Vec<_> = bot.documents.iter().rev().collect();
        documents.sort_by_key(|doc| std::cmp::Reverse(doc.updated_at));
        if !expanded {
            documents.truncate(3);
        }
        div()
            .id("bot-document-list")
            .debug_selector(|| "bot-document-list".into())
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_px()
            .when(bot.documents.is_empty(), |d| {
                d.child(
                    div()
                        .px_2()
                        .py_2()
                        .text_size(px(12.))
                        .text_color(content(0.4))
                        .child("No documents yet"),
                )
            })
            .children(documents.into_iter().map(|document| {
                let id = document.id.clone();
                let bot_id = bot.id.clone();
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
                    .flex_shrink_0()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2p5()
                    .rounded(px(radius::CONTROL))
                    .px_2()
                    .py_2()
                    .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.05)))
                    .child(
                        div()
                            .size(px(28.))
                            .flex_shrink_0()
                            .rounded(px(radius::CONTROL))
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
                        this.set_workspace_context(Some(&bot_id), cx);
                        this.open_document_workspace_pane(&id, cx);
                        cx.stop_propagation();
                    }))
            }))
            .into_any_element()
    }
    pub(crate) fn page_header(&self, title: &str, cx: &mut Context<Self>) -> Div {
        div()
            .h(px(40.))
            .flex_shrink_0()
            .min_w_0()
            .overflow_hidden()
            .border_b_1()
            .border_color(content(0.04))
            .pl_2()
            .pr(px(44.))
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
    pub(crate) fn route_page(
        &mut self,
        bot: &Bot,
        route: Route,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match route {
            Route::Habit(id) => self.habit_page(bot, Some(&id), window, cx),
            Route::NewHabit => self.habit_page(bot, None, window, cx),
        }
    }
}
