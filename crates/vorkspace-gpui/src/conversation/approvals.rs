//! Permission requests use the queued-message shelf and its upward reveal.
use super::*;
use std::cell::RefCell;

#[derive(Default)]
pub(super) struct State {
    current: RefCell<Option<String>>,
    motion: composer_shelf::Motion,
}

impl State {
    fn sync(&self, id: Option<&str>, animate: bool) {
        let mut current = self.current.borrow_mut();
        if current.as_deref() != id {
            *current = id.map(str::to_owned);
            if let Some(id) = id {
                self.motion.insert(id, true, animate);
            }
        }
        self.motion.retain(id.into_iter(), animate);
    }
}

impl Conversation {
    #[cfg(test)]
    pub(crate) fn sidebar_test_approval(&mut self, required: bool, cx: &mut Context<Self>) {
        self.approvals.clear();
        if required {
            self.approvals.push_back(Approval {
                id: "sidebar-fixture".into(),
                title: "Approve fixture command".into(),
                detail: String::new(),
                can_remember: false,
            });
        }
        cx.emit(SessionChanged);
        cx.notify();
    }

    pub(super) fn approval_shelf(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let width = self.composer_anchor.get().map(|bounds| bounds.size.width);
        self.approval_ui.sync(
            self.approvals.front().map(|approval| approval.id.as_str()),
            width.is_some(),
        );
        let Some(approval) = self.approvals.front() else {
            return div().into_any_element();
        };
        let mut ink = shell_fill();
        ink.a = 1.;
        let deny_id = approval.id.clone();
        let session_id = approval.id.clone();
        let allow_id = approval.id.clone();
        let card = composer_shelf::card("approval-request", self.classic_layout)
            .role(Role::Group)
            .aria_label(approval.title.clone())
            .child(
                div()
                    .min_h(px(32.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(icon("square-lock", 14.).text_color(gpui::rgb(0xfbbf24)))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .py_1()
                            .text_color(content(0.75))
                            .child(approval.title.clone()),
                    ),
            )
            .when(!approval.detail.is_empty(), |card| {
                card.child(
                    div()
                        .id("approval-detail")
                        .role(Role::Label)
                        .aria_label(approval.detail.clone())
                        .border_t_1()
                        .border_color(content(0.05))
                        .py_1()
                        .max_h(px(180.))
                        .overflow_y_scroll()
                        .font_family(crate::markdown::FIXED_FONT)
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(content(0.75))
                        .child(approval.detail.clone()),
                )
            })
            .child(
                div()
                    .py_1()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_end()
                    .gap(px(6.))
                    .child(
                        approval_button("approval-deny", "Deny")
                            .text_color(content(0.5))
                            .hover(|s| s.bg(content(0.08)).text_color(content(0.9)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.decide_notification_request(&deny_id, Decision::Deny, cx);
                            })),
                    )
                    .when(approval.can_remember, |d| {
                        d.child(
                            approval_button("approval-session", "Allow for this session")
                                .bg(content(0.06))
                                .hover(|s| s.bg(content(0.1)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.decide_notification_request(
                                        &session_id,
                                        Decision::Session,
                                        cx,
                                    );
                                })),
                        )
                    })
                    .child(
                        approval_button("approval-allow", "Allow once")
                            .bg(content(1.))
                            .text_color(ink)
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .hover(move |s| s.bg(content(0.85)).text_color(ink))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.decide_notification_request(&allow_id, Decision::Allow, cx);
                            })),
                    ),
            );
        let card_width = (width.unwrap_or_default() - px(16.)).max(px(0.));
        composer_shelf::shelf("approval-shelf")
            .debug_selector(|| "approval-shelf".into())
            .child(self.approval_ui.motion.card(card, card_width, window))
            .into_any_element()
    }
}

fn approval_button(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .role(Role::Button)
        .aria_label(label)
        .cursor_pointer()
        .flex_shrink_0()
        .h(px(26.))
        .px(px(10.))
        .rounded_md()
        .flex()
        .items_center()
        .text_size(px(12.))
        .child(label)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

#[cfg(test)]
mod tests;
