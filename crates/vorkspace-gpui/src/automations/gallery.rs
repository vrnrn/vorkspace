use super::*;
use vorkspace_ui::radius;

fn gallery_card(
    id: impl Into<gpui::ElementId>,
    label: &str,
    mark: &'static str,
    description: &str,
    trigger: Option<gpui::Div>,
) -> gpui::Stateful<gpui::Div> {
    crate::controls::button(id, label.to_owned(), true)
        .h_full()
        .min_w_0()
        .p(px(12.))
        .rounded(px(radius::SURFACE))
        .border_1()
        .border_color(page::divider())
        .flex()
        .items_start()
        .gap(px(8.))
        .cursor_pointer()
        .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.05)))
        .child(
            div()
                .size(px(page::CONTROL_HEIGHT))
                .flex_shrink_0()
                .rounded(px(radius::CONTROL))
                .bg(content(0.08))
                .text_color(page::secondary_ink())
                .flex()
                .items_center()
                .justify_center()
                .child(icon(mark, page::ICON_SIZE)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    page::label()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(label.to_owned()),
                )
                .child(page::meta().child(description.to_owned()))
                .children(trigger),
        )
}

impl Automations {
    pub(super) fn render_gallery(&self, narrow: bool, cx: &mut Context<Self>) -> AnyElement {
        let categories = [
            ("popular", "Popular"),
            ("review", "Code Review"),
            ("security", "Security"),
            ("incidents", "Incidents & Triage"),
            ("research", "Data & Research"),
            ("environment", "Environment"),
        ];
        let scratch = gallery_card(
            "automation-scratch",
            "Start from scratch",
            "plus",
            "Write your own instructions and choose a trigger.",
            None,
        )
        .debug_selector(|| "automation-scratch".into())
        .on_click(cx.listener(|this, _, _, cx| this.begin(None, cx)));
        let cards = self
            .templates
            .iter()
            .filter(|template| {
                if self.category == "popular" {
                    template.popular
                } else {
                    template.category == self.category
                }
            })
            .map(|template| {
                let selected = template.clone();
                let path = match template.icon.as_str() {
                    "alert" => "alert-circle",
                    "lock" => "lock",
                    "file" => "file",
                    "check" => "check",
                    "pr" => "git-pull-request",
                    "inbox" => "inbox",
                    "terminal" => "terminal",
                    "note" => "note",
                    "gauge" => "gauge",
                    _ => "search",
                };
                let trigger = page::meta()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(trigger_mark(
                        template.trigger["kind"].as_str().unwrap_or("time"),
                        page::ICON_SIZE,
                    ))
                    .child(template.trigger_label.clone());
                gallery_card(
                    SharedString::from(format!("template-{}", template.id)),
                    &template.name,
                    path,
                    &template.description,
                    Some(trigger),
                )
                .debug_selector(|| format!("automation-template-{}", template.id))
                .on_click(cx.listener(move |this, _, _, cx| this.begin(Some(selected.clone()), cx)))
            });
        // Half-width columns share their side padding. A column that would
        // become too cramped wraps and fills its own line.
        let grid = div().mx(px(-6.)).flex().flex_wrap().children(
            std::iter::once(scratch).chain(cards).map(|card| {
                // Columns never grow, so an odd last card keeps its column's
                // width instead of stretching across the row.
                div()
                    .flex_grow(if narrow { 1. } else { 0. })
                    .flex_basis(gpui::relative(if narrow { 1. } else { 0.5 }))
                    .max_w_full()
                    .when(!narrow, |d| d.min_w(px(264.)))
                    .px(px(6.))
                    .pb(px(12.))
                    .child(card)
            }),
        );
        div()
            .id("automation-gallery")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.gallery_scroll)
            .child(
                detail_frame(narrow)
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(page::title().child("New automation"))
                            .child(
                                page::body()
                                    .text_color(page::secondary_ink())
                                    .child("Pick an example or start from scratch."),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(4.))
                            .children(categories.map(|(id, label)| {
                                page::tab(
                                    SharedString::from(format!("automation-category-{id}")),
                                    label,
                                    self.category == id,
                                )
                                .child(label)
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.category = id.into();
                                        cx.notify();
                                    },
                                ))
                            })),
                    )
                    .child(grid),
            )
            .into_any_element()
    }
}
