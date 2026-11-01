//! Provider-neutral question card, retaining selection and native text fields.
use crate::input;
use gpui::{
    AnyElement, Context, Entity, EventEmitter, MouseButton, Role, Subscription, div, prelude::*, px,
};
use vorkspace_core::agent::Question;
use vorkspace_ui::theme::*;

pub(crate) struct Answered {
    pub id: String,
    pub answers: Option<Vec<Vec<String>>>,
}
pub(crate) struct KeptOpen {
    pub id: String,
}
pub(crate) struct QuestionCard {
    request: Question,
    /// Fixed for the card's life. Interacting clears the deadline, but the
    /// footer row it showed must keep its height: a mouse-down that shrinks the
    /// card moves the pinned options before the click completes.
    optional: bool,
    selected: Vec<Vec<String>>,
    fields: Vec<Entity<input::Input>>,
    _events: Vec<Subscription>,
    _clock: Option<gpui::Task<()>>,
}
impl EventEmitter<Answered> for QuestionCard {}
impl EventEmitter<KeptOpen> for QuestionCard {}
impl QuestionCard {
    pub(crate) fn new(request: Question, cx: &mut Context<Self>) -> Self {
        let fields: Vec<_> = request
            .items
            .iter()
            .map(|item| {
                cx.new(|cx| input::Input::field(&item.prompt, "Type an answer…", content(0.4), cx))
            })
            .collect();
        let events = fields
            .iter()
            .map(|field| {
                cx.subscribe(field, |this, _, _: &input::Changed, cx| {
                    this.keep_open(cx);
                    cx.notify();
                })
            })
            .collect();
        let selected = vec![Vec::new(); request.items.len()];
        let clock = request.auto_resolve_at.map(|_| {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(1))
                        .await;
                    if !this
                        .update(cx, |this, cx| {
                            cx.notify();
                            this.request.auto_resolve_at.is_some()
                        })
                        .unwrap_or(false)
                    {
                        break;
                    }
                }
            })
        });
        Self {
            optional: request.auto_resolve_at.is_some(),
            request,
            selected,
            fields,
            _events: events,
            _clock: clock,
        }
    }
    pub(crate) fn id(&self) -> &str {
        &self.request.id
    }
    pub(crate) fn blocks_steering(&self) -> bool {
        self.request.auto_resolve_at.is_none()
    }
    fn keep_open(&mut self, cx: &mut Context<Self>) {
        if self.request.auto_resolve_at.take().is_some() {
            cx.emit(KeptOpen {
                id: self.request.id.clone(),
            });
            cx.notify();
        }
    }
    pub(crate) fn notification_label(&self) -> &str {
        if !self.request.title.is_empty() {
            &self.request.title
        } else {
            self.request
                .items
                .first()
                .map(|item| item.prompt.as_str())
                .unwrap_or("Question")
        }
    }
    fn answers(&self, cx: &gpui::App) -> Vec<Vec<String>> {
        self.request
            .items
            .iter()
            .enumerate()
            .map(|(ix, item)| {
                let mut answers = self.selected[ix].clone();
                let text = self.fields[ix].read(cx).content.trim();
                if item.allow_custom && !text.is_empty() {
                    if !item.multiple {
                        answers.clear();
                    }
                    answers.push(text.to_owned());
                }
                answers
            })
            .collect()
    }
}
impl Render for QuestionCard {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let items: Vec<AnyElement> = self
            .request
            .items
            .iter()
            .enumerate()
            .map(|(ix, item)| {
                let options: Vec<AnyElement> = item
                    .options
                    .iter()
                    .enumerate()
                    .map(|(oi, option)| {
                        let label = option.label.clone();
                        let selected = self.selected[ix].contains(&label);
                        let multiple = item.multiple;
                        div()
                            .id(("question-option", ix * 1024 + oi))
                            .debug_selector(move || format!("question-option-{ix}-{oi}"))
                            .role(if multiple {
                                Role::CheckBox
                            } else {
                                Role::RadioButton
                            })
                            .aria_label(label.clone())
                            .aria_selected(selected)
                            .rounded_md()
                            .px_2()
                            .py_1()
                            .border_1()
                            .border_color(content(if selected { 0.4 } else { 0.12 }))
                            .bg(content(if selected { 0.12 } else { 0.03 }))
                            .hover(|s| s.bg(content(0.1)))
                            .child(label.clone())
                            .when(!option.description.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(content(0.55))
                                        .child(option.description.clone()),
                                )
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.keep_open(cx);
                                if this.selected[ix].contains(&label) {
                                    this.selected[ix].retain(|s| s != &label);
                                } else {
                                    if !multiple {
                                        this.selected[ix].clear();
                                    }
                                    this.selected[ix].push(label.clone());
                                }
                                this.fields[ix]
                                    .update(cx, |field, cx| field.set_text(String::new(), cx));
                                cx.notify();
                            }))
                            .into_any_element()
                    })
                    .collect();
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(item.prompt.clone())
                    .children(options)
                    .when(item.allow_custom, |d| {
                        d.child(
                            div()
                                .mt_1()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(content(0.05))
                                .child(self.fields[ix].clone()),
                        )
                    })
                    .into_any_element()
            })
            .collect();
        let answers = self.answers(cx);
        let complete = answers.iter().all(|a| !a.is_empty());
        div()
            .id("provider-question")
            .role(Role::Group)
            .aria_label(self.request.title.clone())
            .mb_2()
            .p_3()
            .rounded(px(12.))
            .border_1()
            .border_color(content(0.15))
            .bg(content(0.04))
            .text_size(px(13.))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.keep_open(cx)),
            )
            .child(
                div()
                    .mb_2()
                    .text_size(px(14.))
                    .child(self.request.title.clone()),
            )
            .child(
                div()
                    .id("provider-question-items")
                    .max_h(px(300.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children(items),
            )
            .when(self.optional, |card| {
                let status = match self.request.auto_resolve_at {
                    Some(deadline) => {
                        let remaining = deadline.saturating_sub(vorkspace_core::time::now_ms());
                        if remaining > 60_000 {
                            "Optional question".to_owned()
                        } else {
                            format!(
                                "Continues without an answer in {}s",
                                remaining.div_ceil(1000)
                            )
                        }
                    }
                    None => "Waiting for your answer".to_owned(),
                };
                card.child(
                    div()
                        .mt_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(px(11.))
                        .text_color(content(0.5))
                        .child(status),
                )
            })
            .child(
                div()
                    .mt_2()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .id("question-skip")
                            .role(Role::Button)
                            .aria_label("Skip question")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(content(0.06))
                            .child("Skip")
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.emit(Answered {
                                    id: this.request.id.clone(),
                                    answers: None,
                                })
                            })),
                    )
                    .child(
                        div()
                            .id("question-answer")
                            .debug_selector(|| "question-answer".into())
                            .role(Role::Button)
                            .aria_label("Submit answers")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(content(if complete { 0.14 } else { 0.03 }))
                            .opacity(if complete { 1. } else { 0.4 })
                            .child("Submit answers")
                            .when(complete, |d| {
                                d.on_click(cx.listener(|this, _, _, cx| {
                                    cx.emit(Answered {
                                        id: this.request.id.clone(),
                                        answers: Some(this.answers(cx)),
                                    })
                                }))
                            }),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};
    use std::{cell::RefCell, rc::Rc};
    use vorkspace_core::agent::{QuestionItem, QuestionOption};
    #[gpui::test]
    fn question_card_requires_answers_and_preserves_multiple_choices_and_custom_text(
        cx: &mut TestAppContext,
    ) {
        let request = Question {
            id: "r".into(),
            title: "A question".into(),
            auto_resolve_at: None,
            items: vec![
                QuestionItem {
                    id: "one".into(),
                    prompt: "Choose one".into(),
                    multiple: false,
                    allow_custom: true,
                    options: vec![
                        QuestionOption {
                            id: "a".into(),
                            label: "First".into(),
                            description: String::new(),
                        },
                        QuestionOption {
                            id: "b".into(),
                            label: "Second".into(),
                            description: String::new(),
                        },
                    ],
                },
                QuestionItem {
                    id: "many".into(),
                    prompt: "Choose several".into(),
                    multiple: true,
                    allow_custom: true,
                    options: vec![
                        QuestionOption {
                            id: "a".into(),
                            label: "A".into(),
                            description: String::new(),
                        },
                        QuestionOption {
                            id: "b".into(),
                            label: "B".into(),
                            description: String::new(),
                        },
                    ],
                },
            ],
        };
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| QuestionCard::new(request, cx))
            })
            .unwrap()
        });
        let replies = Rc::new(RefCell::new(Vec::new()));
        let output = replies.clone();
        let _events = window
            .update(cx, |_, _, cx| {
                cx.subscribe(&cx.entity(), move |_, _, reply: &Answered, _| {
                    output.borrow_mut().push(reply.answers.clone())
                })
            })
            .unwrap();
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.refresh().unwrap();
        let submit = visual.debug_bounds("question-answer").unwrap();
        visual.simulate_click(submit.center(), Default::default());
        assert!(
            replies.borrow().is_empty(),
            "an incomplete answer cannot be submitted"
        );
        for selector in [
            "question-option-0-0",
            "question-option-0-1",
            "question-option-1-0",
            "question-option-1-1",
        ] {
            let bounds = visual.debug_bounds(selector).unwrap();
            visual.simulate_click(bounds.center(), Default::default());
            visual.refresh().unwrap();
        }
        window
            .update(visual, |card, _, cx| {
                assert_eq!(card.answers(cx), vec![vec!["Second"], vec!["A", "B"]]);
                card.fields[0].update(cx, |field, cx| field.set_text("Custom one".into(), cx));
                card.fields[1].update(cx, |field, cx| field.set_text("Custom extra".into(), cx));
                cx.notify();
            })
            .unwrap();
        visual.refresh().unwrap();
        let submit = visual.debug_bounds("question-answer").unwrap();
        visual.simulate_click(submit.center(), Default::default());
        assert_eq!(
            *replies.borrow(),
            vec![Some(vec![
                vec!["Custom one".to_owned()],
                vec!["A".to_owned(), "B".to_owned(), "Custom extra".to_owned()]
            ])]
        );
    }

    #[gpui::test]
    fn pressing_an_option_of_an_optional_question_does_not_move_the_card(cx: &mut TestAppContext) {
        let request = Question {
            id: "r".into(),
            title: "Optional".into(),
            auto_resolve_at: Some(vorkspace_core::time::now_ms() + 600_000),
            items: vec![QuestionItem {
                id: "one".into(),
                prompt: "Choose".into(),
                multiple: false,
                allow_custom: false,
                options: vec![QuestionOption {
                    id: "a".into(),
                    label: "First".into(),
                    description: String::new(),
                }],
            }],
        };
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| QuestionCard::new(request, cx))
            })
            .unwrap()
        });
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.refresh().unwrap();
        let option = visual.debug_bounds("question-option-0-0").unwrap();
        let submit = visual.debug_bounds("question-answer").unwrap();
        visual.simulate_mouse_down(option.center(), gpui::MouseButton::Left, Default::default());
        visual.refresh().unwrap();
        window
            .update(visual, |card, _, _| {
                assert!(card.request.auto_resolve_at.is_none())
            })
            .unwrap();
        assert_eq!(visual.debug_bounds("question-option-0-0"), Some(option));
        assert_eq!(visual.debug_bounds("question-answer"), Some(submit));
    }
}
