use super::*;

/// A short plain-text excerpt; formatting and paragraph gaps belong in details.
fn description_preview(description: &str) -> String {
    use pulldown_cmark::{Event, Parser};
    let end = description
        .char_indices()
        .nth(1024)
        .map_or(description.len(), |(at, _)| at);
    let mut text = String::new();
    for event in Parser::new(&description[..end]) {
        match event {
            Event::Text(value) | Event::Code(value) => text.push_str(&value),
            Event::End(_) | Event::SoftBreak | Event::HardBreak => text.push(' '),
            _ => (),
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl TodoBoardView {
    pub(super) fn row(&self, item: &TodoItem, cx: &mut Context<Self>) -> gpui::AnyElement {
        let status = item.status;
        let before = item.id.clone();
        let dragged = DraggedTodo {
            board: self.board.as_ref().map(TodoBoard::key).unwrap_or_default(),
            id: item.id.clone(),
            title: item.title.clone().into(),
        };
        let open = item.clone();
        let keyboard = item.clone();
        div()
            .id(SharedString::from(format!("todo-{}", item.id)))
            .debug_selector({
                let id = item.id.clone();
                move || format!("todo-{id}")
            })
            .role(Role::Button)
            .aria_label(format!("{}, {}. Open details", item.title, status.label()))
            .focusable()
            .tab_stop(true)
            .group("todo-card")
            .w_full()
            .min_w_0()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .rounded(px(radius::CONTAINER))
            .border_1()
            .border_color(content(0.1))
            .bg(content(0.035))
            .cursor_pointer()
            .hover(|s| s.bg(content(0.065)).border_color(content(0.18)))
            .focus_visible(|s| s.border_color(accent_color()))
            .drag_over::<DraggedTodo>(|s, _, _, _| s.border_color(accent_color()))
            .on_drop(cx.listener(move |this, dragged: &DraggedTodo, _, cx| {
                this.move_to(dragged, status, Some(before.clone()), cx)
            }))
            .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.present_details(&open, window, cx);
            }))
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        this.present_details(&keyboard, window, cx);
                        cx.stop_propagation();
                    }
                }),
            )
            .child(
                div()
                    .text_size(px(page::LABEL_SIZE))
                    .line_height(px(page::LABEL_LINE))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(content(0.95))
                    .line_clamp(3)
                    .text_ellipsis()
                    .child(item.title.clone()),
            )
            .when(!item.description.trim().is_empty(), |d| {
                d.child(
                    div()
                        .debug_selector(|| "todo-description-preview".into())
                        .text_size(px(page::META_SIZE))
                        .line_height(px(20.))
                        .text_color(page::secondary_ink())
                        .line_clamp(3)
                        .text_ellipsis()
                        .child(description_preview(&item.description)),
                )
            })
            .when(!item.dependencies.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .pt_1()
                        .children(item.dependencies.iter().take(2).map(|reference| {
                            let target = self.dependency_item(reference).cloned();
                            div()
                                .id(SharedString::from(format!(
                                    "dependency-{}-{reference}",
                                    item.id
                                )))
                                .flex()
                                .items_center()
                                .gap_1()
                                .min_w_0()
                                .text_size(px(11.))
                                .text_color(page::secondary_ink())
                                .child(icon("git-branch", 12.))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .child(self.dependency_label(reference)),
                                )
                                .when_some(target, |d, target| {
                                    d.role(Role::Button)
                                        .aria_label(format!("Depends on {}", target.title))
                                        .hover(|s| s.text_color(accent_color()))
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation()
                                        })
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.present_details(&target, window, cx);
                                        }))
                                })
                        }))
                        .when(item.dependencies.len() > 2, |d| {
                            d.child(
                                page::meta().child(format!(
                                    "+{} dependencies",
                                    item.dependencies.len() - 2
                                )),
                            )
                        }),
                )
            })
            .into_any_element()
    }

    pub(super) fn dependency_item(&self, reference: &str) -> Option<&TodoItem> {
        self.items.iter().find(|item| {
            item.id == reference
                || Self::dependency_ref(item) == reference
                || item
                    .issue
                    .as_ref()
                    .is_some_and(|issue| issue.url == reference)
        })
    }
}
