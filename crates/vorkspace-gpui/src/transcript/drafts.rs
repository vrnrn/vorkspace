use super::*;
use gpui::{Entity, Focusable, PathBuilder, Subscription, point};
use vorkspace_core::follow_up::Message as Draft;

pub(super) struct DraftEdit {
    id: String,
    edit_id: String,
    original: String,
    input: Entity<crate::input::Input>,
    _submit: Subscription,
    _observe: Subscription,
}

impl TranscriptView {
    pub(crate) fn set_draft_transfer(&mut self, retry: Option<bool>, cx: &mut Context<Self>) {
        self.draft_transfer = retry;
        cx.notify();
    }
    pub(crate) fn set_remote_saved_draft(&mut self, draft: Option<Draft>, cx: &mut Context<Self>) {
        self.draft_editable = false;
        self.set_saved_draft(draft, cx);
    }
    pub(crate) fn finish_saved_draft_edit(&mut self, draft: Option<Draft>, cx: &mut Context<Self>) {
        self.draft_edit = None;
        self.set_saved_draft(draft, cx);
        cx.notify();
    }
    pub(crate) fn update_saved_draft_edit(&mut self, draft: Option<Draft>) {
        if let (Some(edit), Some(draft)) = (&mut self.draft_edit, &draft) {
            edit.id = draft.id.clone();
        }
        self.saved_draft = draft;
    }
    pub(crate) fn is_saved_draft_edit(&self, id: &str, edit_id: &str) -> bool {
        self.draft_edit
            .as_ref()
            .is_some_and(|edit| edit.id == id && edit.edit_id == edit_id)
    }
    pub(crate) fn has_saved_draft_edit(&self, id: &str) -> bool {
        self.draft_edit.as_ref().is_some_and(|edit| edit.id == id)
    }
    pub(crate) fn saved_draft_edit_matches(&self, text: &str, cx: &gpui::App) -> bool {
        self.draft_edit
            .as_ref()
            .is_some_and(|edit| edit.input.read(cx).content == text)
    }
    fn begin_saved_draft_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.saved_draft.as_ref() else {
            return;
        };
        let id = draft.id.clone();
        let text = draft.text.clone();
        let original = text.clone();
        let input = cx.new(|cx| {
            let mut input = crate::input::Input::queued_message(cx);
            input.set_text(text, cx);
            input
        });
        let submit = cx.subscribe_in(
            &input,
            window,
            |this, _, _: &crate::input::Submitted, _, cx| {
                this.submit_saved_draft_edit(true, cx);
            },
        );
        let observe = cx.observe(&input, |this, _, cx| {
            this.submit_saved_draft_edit(false, cx)
        });
        window.focus(&input.focus_handle(cx), cx);
        self.draft_edit = Some(DraftEdit {
            id,
            edit_id: vorkspace_core::bot::new_id(),
            original,
            input,
            _submit: submit,
            _observe: observe,
        });
        cx.notify();
    }
    fn submit_saved_draft_edit(&self, finish: bool, cx: &mut Context<Self>) {
        let Some(edit) = self.draft_edit.as_ref() else {
            return;
        };
        if edit.input.read(cx).is_composing() {
            return;
        }
        let text = edit.input.read(cx).content.to_string();
        if text.trim().is_empty()
            && self
                .saved_draft
                .as_ref()
                .is_none_or(|draft| draft.attachments.is_empty())
        {
            return;
        }
        cx.emit(ResponseRequest::EditDraft {
            id: edit.id.clone(),
            edit_id: edit.edit_id.clone(),
            text,
            finish,
            cancel: false,
        });
    }
    fn cancel_saved_draft_edit(&self, cx: &mut Context<Self>) {
        let Some(edit) = self.draft_edit.as_ref() else {
            return;
        };
        if edit.input.read(cx).is_composing() {
            return;
        }
        cx.emit(ResponseRequest::EditDraft {
            id: edit.id.clone(),
            edit_id: edit.edit_id.clone(),
            text: edit.original.clone(),
            finish: true,
            cancel: true,
        });
    }
    /// Saving a new draft is a user submission, even though no provider starts.
    /// Cold restores and persistence updates use set_saved_draft without replay.
    pub(crate) fn introduce_saved_draft(&mut self, draft: Draft, cx: &mut Context<Self>) {
        if !self.bot {
            self.follow = Follow::default();
        }
        if self.anchor_prompts && !self.bot {
            self.anchor_turn = Some(SAVED_DRAFT_START);
            self.show_jump = false;
            if self.visible && self.chat_layout {
                self.prompt_rise = Some((SAVED_DRAFT_START, Instant::now()));
                self.prompt_rise_pending = true;
            }
        }
        self.list.set_follow_mode(FollowMode::Tail);
        self.set_saved_draft(Some(draft), cx);
        self.list.scroll_to_end();
    }

    pub(crate) fn set_saved_draft(&mut self, draft: Option<Draft>, cx: &mut Context<Self>) {
        if self
            .draft_edit
            .as_ref()
            .is_some_and(|edit| draft.as_ref().is_none_or(|draft| draft.id != edit.id))
        {
            self.draft_edit = None;
        }
        if self.saved_draft == draft {
            return;
        }
        self.saved_draft = draft;
        if self.saved_draft.is_none() && self.anchor_turn == Some(SAVED_DRAFT_START) {
            self.anchor_turn = self.source.iter().rposition(|row| row.is_prompt());
            self.prompt_rise = None;
            self.prompt_rise_pending = false;
        }
        if self.source.is_empty() {
            self.rows.clear();
            self.push_saved_draft();
            self.list.reset(self.rows.len() + 1);
            self.list.scroll_to_end();
        } else {
            self.rebuild(0, cx);
        }
        cx.notify();
    }
    pub(super) fn push_saved_draft(&mut self) {
        if let Some(draft) = &self.saved_draft {
            if self
                .rows
                .iter()
                .any(|row| matches!(row.body, Body::SavedDraft))
            {
                return;
            }
            let index = self
                .rows
                .iter()
                .position(|row| row.key.0 >= draft.at)
                .unwrap_or(self.rows.len());
            self.rows.insert(
                index,
                UiRow {
                    key: (SAVED_DRAFT_START, 0),
                    body: Body::SavedDraft,
                    message: usize::MAX,
                    first: false,
                    last: false,
                    work: false,
                    work_tail: false,
                    thought_phase: None,
                },
            );
        }
    }
    pub(super) fn saved_draft_row(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(draft) = &self.saved_draft else {
            return div().into_any_element();
        };
        let (mut rise, mut opacity) = (0., 1.);
        if let Some((SAVED_DRAFT_START, born)) = self.prompt_rise {
            let elapsed = born.elapsed().as_secs_f32();
            let progress = crate::reveal::ease_move((elapsed / 0.56).min(1.));
            rise =
                (f32::from(self.viewport.size.height) * 0.3 - TOP_INSET).max(0.) * (1. - progress);
            opacity = crate::reveal::ease_out((elapsed / 0.48).min(1.));
        }
        div()
            .id("saved-message-draft")
            .w_full()
            .flex()
            .justify_center()
            .opacity(opacity)
            .child(
                div()
                    .relative()
                    .top(px(rise))
                    .w_full()
                    .max_w(px(COLUMN))
                    .px(px(32.))
                    .pt(px(12.))
                    .pb(px(20.))
                    .flex()
                    .justify_end()
                    .child(
                        div()
                            .relative()
                            .min_w_0()
                            .max_w(px(576.))
                            .rounded(px(12.))
                            .border_1()
                            .border_color(content(0.))
                            .bg(content(0.04))
                            .px_3()
                            .py_2()
                            .text_size(px(14.))
                            .text_color(content(1.))
                            .line_height(px(20.))
                            .child(draft_dashes(Some(12.), 0.3))
                            .children(draft.attachments.iter().map(|path| {
                                div()
                                    .mb_1()
                                    .text_size(px(12.))
                                    .text_color(content(0.65))
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .child(icon("file", 14.))
                                    .child(
                                        path.file_name()
                                            .unwrap_or_default()
                                            .to_string_lossy()
                                            .to_string(),
                                    )
                            }))
                            .map(|card| {
                                if let Some(edit) = &self.draft_edit {
                                    card.child(
                                        div()
                                            .id("saved-draft-editor")
                                            .min_w(px(120.))
                                            .max_w_full()
                                            .rounded_md()
                                            .border_1()
                                            .border_color(content(0.3))
                                            .px_2()
                                            .py_1()
                                            .child(edit.input.clone()),
                                    )
                                } else {
                                    card.child(
                                        div()
                                            .id("saved-draft-text")
                                            .role(Role::Label)
                                            .aria_label(draft.text.clone())
                                            .child(draft.text.clone()),
                                    )
                                }
                            })
                            .child(
                                div()
                                    .relative()
                                    .mt_2()
                                    .pt_2()
                                    .border_t_1()
                                    .border_color(content(0.))
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .child(draft_dashes(None, 0.2))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.))
                                            .text_size(px(12.))
                                            .text_color(content(0.5))
                                            .child(icon("circle-dashed-hi", 14.))
                                            .child("Draft"),
                                    )
                                    .child(div().flex().flex_wrap().items_center().gap_1().map(
                                        |actions| {
                                            if self.draft_edit.is_some() {
                                                actions
                                                    .child(
                                                        draft_button(
                                                            "cancel-draft-edit",
                                                            "Cancel draft edit",
                                                        )
                                                        .px_2()
                                                        .child("Cancel")
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.cancel_saved_draft_edit(cx)
                                                        })),
                                                    )
                                                    .child(
                                                        draft_button(
                                                            "save-draft-edit",
                                                            "Save draft edit",
                                                        )
                                                        .px_2()
                                                        .child("Save")
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.submit_saved_draft_edit(true, cx)
                                                        })),
                                                    )
                                            } else {
                                                actions
                                                    .when(
                                                        self.draft_editable
                                                            && self.draft_transfer.is_none(),
                                                        |actions| {
                                                            actions.child(
                                                                draft_button(
                                                                    "edit-message-draft",
                                                                    "Edit draft",
                                                                )
                                                                .px_2()
                                                                .text_color(content(0.55))
                                                                .child(icon("pencil-edit", 14.))
                                                                .child("Edit")
                                                                .on_click(cx.listener(
                                                                    |this, _, window, cx| {
                                                                        this.begin_saved_draft_edit(
                                                                            window, cx,
                                                                        )
                                                                    },
                                                                )),
                                                            )
                                                        },
                                                    )
                                                    .when(
                                                        self.draft_transfer.is_none(),
                                                        |actions| {
                                                            actions.child(
                                                        draft_button(
                                                            "remove-message-draft",
                                                            "Remove draft",
                                                        )
                                                        .px_2()
                                                        .text_color(content(0.55))
                                                        .child(icon("trash", 14.))
                                                        .child("Remove")
                                                        .on_click(cx.listener(|_, _, _, cx| {
                                                            cx.emit(ResponseRequest::Draft {
                                                                send: false,
                                                            })
                                                        })),
                                                    )
                                                        },
                                                    )
                                                    .when(
                                                        self.draft_transfer != Some(false),
                                                        |actions| {
                                                            actions.child(
                                                            draft_button(
                                                                "send-message-draft",
                                                                "Send draft",
                                                            )
                                                            .px(px(10.))
                                                            .bg(white(1.))
                                                            .text_color(black(1.))
                                                            .child("Send")
                                                            .child(
                                                                icon("send", 14.)
                                                                    .text_color(black(1.)),
                                                            )
                                                            .on_click(cx.listener(|_, _, _, cx| {
                                                                cx.emit(ResponseRequest::Draft {
                                                                    send: true,
                                                                })
                                                            })),
                                                        )
                                                        },
                                                    )
                                            }
                                        },
                                    )),
                            ),
                    ),
            )
            .into_any_element()
    }
}

/// Browser dashed 1px borders use approximately 3px dashes with 3px gaps.
/// GPUI's built-in 2px/1px pattern is visibly tighter. Keep the browser pattern
/// local to draft bubbles and their divider, including the rounded corners.
fn draft_dashes(radius: Option<f32>, alpha: f32) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let left = bounds.left() + px(0.5);
            let right = bounds.right() - px(0.5);
            let top = bounds.top() + px(0.5);
            let bottom = bounds.bottom() - px(0.5);
            let mut path = PathBuilder::stroke(px(1.)).dash_array(&[px(3.), px(3.)]);
            if let Some(radius) = radius {
                let r = px((radius - 0.5).max(0.))
                    .min((right - left) / 2.)
                    .min((bottom - top) / 2.);
                let radii = point(r, r);
                path.move_to(point(left + r, top));
                path.line_to(point(right - r, top));
                path.arc_to(radii, px(0.), false, true, point(right, top + r));
                path.line_to(point(right, bottom - r));
                path.arc_to(radii, px(0.), false, true, point(right - r, bottom));
                path.line_to(point(left + r, bottom));
                path.arc_to(radii, px(0.), false, true, point(left, bottom - r));
                path.line_to(point(left, top + r));
                path.arc_to(radii, px(0.), false, true, point(left + r, top));
                path.close();
            } else {
                path.move_to(point(left, top));
                path.line_to(point(right, top));
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, content(alpha));
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}
fn draft_button(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .role(Role::Button)
        .aria_label(label)
        .h(px(28.))
        .rounded_md()
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(px(12.))
        .hover(|s| s.bg(content(0.1)))
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn new_drafts_anchor_like_sent_prompts_and_keep_the_anchor_through_updates_and_resize(
        cx: &mut TestAppContext,
    ) {
        for history in [false, true] {
            let window = cx.update(|cx| {
                cx.open_window(
                    gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(Bounds::new(
                            point(px(0.), px(0.)),
                            size(px(900.), px(600.)),
                        ))),
                        ..Default::default()
                    },
                    |_, cx| {
                        cx.new(|cx| {
                            let mut view = TranscriptView::new();
                            if history {
                                let mut transcript =
                                    vorkspace_core::transcript::Transcript::default();
                                transcript.begin("user");
                                transcript.append("An earlier question");
                                transcript.begin("assistant");
                                transcript.append("An earlier answer");
                                view.patch(0, transcript.rows, cx);
                            }
                            view
                        })
                    },
                )
                .unwrap()
            });
            let cx = &mut gpui::VisualTestContext::from_window(window.into(), cx);
            cx.refresh().unwrap();
            window
                .update(cx, |view, _, cx| {
                    view.introduce_saved_draft(
                        Draft {
                            id: "new".into(),
                            text: "Save this for later".into(),
                            at: view.source.len(),
                            ..Default::default()
                        },
                        cx,
                    );
                    assert!(
                        !view.streaming,
                        "saving a draft must not start a provider turn"
                    );
                    assert_eq!(view.prompt_rise.map(|(at, _)| at), Some(SAVED_DRAFT_START));
                    // Measure the final position independently of animation timing.
                    view.prompt_rise = None;
                })
                .unwrap();
            cx.refresh().unwrap();
            cx.run_until_parked();
            let assert_anchor = |view: &TranscriptView| {
                let ix = view
                    .rows
                    .iter()
                    .position(|row| matches!(row.body, Body::SavedDraft))
                    .unwrap();
                let bounds = view.list.bounds_for_item(ix).unwrap();
                assert!(
                    (bounds.top() - view.viewport.top()).abs() < px(1.),
                    "a newly saved draft must sit at the same top anchor as a sent prompt: {bounds:?} {:?}",
                    view.viewport
                );
                assert!(view.list.is_following_tail());
            };
            window
                .update(cx, |view, window, cx| {
                    assert_anchor(view);
                    let mut draft = view.saved_draft.clone().unwrap();
                    draft.attachments = vec!["/saved/attachment.txt".into()];
                    view.set_saved_draft(Some(draft), cx);
                    window.resize(size(px(700.), px(760.)));
                    cx.notify();
                })
                .unwrap();
            cx.refresh().unwrap();
            cx.run_until_parked();
            window
                .update(cx, |view, _, cx| {
                    assert_anchor(view);
                    view.visible(false, cx);
                    view.visible(true, cx);
                    assert_eq!(view.anchor_turn, Some(SAVED_DRAFT_START));
                    view.set_saved_draft(None, cx);
                    assert_ne!(view.anchor_turn, Some(SAVED_DRAFT_START));
                    assert_eq!(view.source.len(), if history { 2 } else { 0 });
                })
                .unwrap();
        }
    }

    #[gpui::test]
    fn restored_drafts_do_not_replay_submission_or_override_the_anchor_preference(
        cx: &mut TestAppContext,
    ) {
        let view = cx.new(|_| TranscriptView::new());
        view.update(cx, |view, cx| {
            view.set_saved_draft(
                Some(Draft {
                    id: "restored".into(),
                    text: "already saved".into(),
                    ..Default::default()
                }),
                cx,
            );
            assert!(view.anchor_turn.is_none());
            assert!(view.prompt_rise.is_none());
            view.set_saved_draft(None, cx);
            view.apply_preferences(
                &crate::settings::Preferences {
                    anchor_prompts: false,
                    ..Default::default()
                },
                cx,
            );
            view.introduce_saved_draft(
                Draft {
                    id: "new".into(),
                    text: "saved without anchoring".into(),
                    ..Default::default()
                },
                cx,
            );
            assert!(view.anchor_turn.is_none());
            assert!(view.prompt_rise.is_none());
        });
    }

    #[gpui::test]
    fn saved_draft_keeps_its_position_as_history_grows_and_removal_leaves_history_intact(
        cx: &mut TestAppContext,
    ) {
        let view = cx.new(|_| TranscriptView::new());
        let row = |role: &str, text: &str| Row {
            role: role.into(),
            text: text.into(),
            first: true,
            sent_at: None,
        };
        view.update(cx, |view, cx| {
            view.patch(
                0,
                vec![row("user", "first"), row("assistant", "answer")],
                cx,
            );
            view.set_saved_draft(
                Some(Draft {
                    id: "draft".into(),
                    text: "for later".into(),
                    at: 2,
                    ..Default::default()
                }),
                cx,
            );
            view.patch(
                2,
                vec![
                    row("user", "another message"),
                    row("assistant", "another answer"),
                ],
                cx,
            );
            let draft = view
                .rows
                .iter()
                .position(|r| matches!(r.body, Body::SavedDraft))
                .unwrap();
            let next = view.rows.iter().position(|r| r.key.0 == 2).unwrap();
            assert!(draft < next);
            assert_eq!(
                view.rows
                    .iter()
                    .filter(|r| matches!(r.body, Body::SavedDraft))
                    .count(),
                1
            );
            let history = view.source.clone();
            view.set_saved_draft(None, cx);
            assert!(
                view.rows
                    .iter()
                    .all(|r| !matches!(r.body, Body::SavedDraft))
            );
            assert_eq!(view.source, history);
        });
    }
}
