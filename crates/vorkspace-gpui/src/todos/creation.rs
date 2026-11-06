use super::*;
use crate::popover::{Align, Side};

impl TodoBoardView {
    fn dismiss_creation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.create_open = false;
        window.focus(&self.create_focus, cx);
        cx.notify();
    }

    pub(super) fn create_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let disabled = self.board.is_none();
        let mut ink = shell_fill();
        ink.a = 1.;
        page::primary("todo-create", "Create task", disabled)
            .debug_selector(|| "todo-create".into())
            .track_focus(&self.create_focus)
            .absolute()
            .right_4()
            .bottom_4()
            .h(px(36.))
            .px_4()
            .gap_2()
            .rounded(px(radius::CONTAINER))
            .shadow_md()
            .child(icon("plus", page::ICON_SIZE).text_color(ink))
            .child("Create task")
            .child(self.create_anchor.probe())
            .when(!disabled, |d| {
                d.on_click(cx.listener(|this, _, window, cx| {
                    if this.create_open {
                        this.dismiss_creation(window, cx);
                    } else {
                        this.create_open = true;
                        window.focus(&this.draft.focus_handle(cx), cx);
                        cx.notify();
                    }
                }))
            })
    }

    pub(super) fn create_popover(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if !self.create_open {
            return None;
        }
        let weak = cx.weak_entity();
        Some(
            popover(
                "todo-create-popover",
                Anchor::Slot(self.create_anchor.clone()),
            )
            .side(Side::Top)
            .align(Align::End)
            .width(320.)
            .padding(12.)
            .ignore(self.create_anchor.clone())
            .on_dismiss(move |window, cx| {
                let _ = weak.update(cx, |this, cx| this.dismiss_creation(window, cx));
            })
            .child(
                div()
                    .id("todo-create-form")
                    .debug_selector(|| "todo-create-form".into())
                    .flex()
                    .flex_col()
                    .gap_3()
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                        if event.keystroke.key == "escape" {
                            this.dismiss_creation(window, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(page::label().child("Create task"))
                    .child(
                        div()
                            .h(px(32.))
                            .px_2()
                            .flex()
                            .items_center()
                            .rounded(px(radius::CONTROL))
                            .border_1()
                            .border_color(page::divider())
                            .child(self.draft.clone()),
                    )
                    .children(self.change_error.as_ref().map(|error| {
                        page::meta()
                            .text_color(status(StatusColor::Danger, 1.))
                            .child(error.clone())
                    }))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                labeled_secondary("todo-create-cancel", "Cancel", false, false)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.dismiss_creation(window, cx)
                                    })),
                            )
                            .child(
                                labeled_primary("todo-create-submit", "Create task", false)
                                    .debug_selector(|| "todo-create-submit".into())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.add(cx);
                                        if !this.create_open {
                                            window.focus(&this.create_focus, cx);
                                        }
                                    })),
                            ),
                    ),
            )
            .into_any_element(),
        )
    }
}
