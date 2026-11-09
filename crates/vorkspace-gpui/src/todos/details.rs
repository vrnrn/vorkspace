use super::*;

impl TodoBoardView {
    pub(super) fn present_details(
        &mut self,
        item: &TodoItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.details.is_none() {
            self.detail_return_focus = window.focused(cx);
        }
        self.open_details(item, cx);
        window.focus(&self.detail_focus, cx);
    }

    pub(super) fn dismiss_details(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stash_details();
        if let Some(focus) = self.detail_return_focus.take() {
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    pub(super) fn details_overlay(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let editor = self.details.as_ref()?;
        let item = self.details_item(editor)?;
        let size = window.viewport_size();

        let height = 680_f32.min((size.height.as_f32() - 40.).max(0.));
        let error = self.change_error.as_ref().or(self.error.as_ref());
        // Reading a to-do fits the dialog to it; editing keeps the full
        // height the editor needs. Either way the top edge stays put.
        let editing = self.detail_editing || item.pending_edit.is_some();
        let width = (if editing { 760_f32 } else { 600. }).min((size.width.as_f32() - 32.).max(0.));
        let body = if editing {
            self.editor_panel(cx)?
        } else {
            self.details_preview(item, editor, cx)
        };
        let weak = cx.weak_entity();
        Some(
            div()
                .absolute()
                .size_full()
                .child(
                    gpui::deferred(
                        gpui::anchored()
                            .position(gpui::point(px(0.), px(0.)))
                            .child(
                                div()
                                    .w(size.width)
                                    .h(size.height)
                                    .bg(vorkspace_ui::fluid::scrim(0.4))
                                    .occlude(),
                            ),
                    )
                    .with_priority(69),
                )
                .child(
                    popover(
                        "todo-details-modal",
                        Anchor::Point(gpui::point(
                            px((size.width.as_f32() - width) / 2.),
                            px((size.height.as_f32() - height) / 2.),
                        )),
                    )
                    .modal_surface(true)
                    .without_animation()
                    .priority(70)
                    .width(width)
                    .map(|p| {
                        if editing {
                            p.height(height)
                        } else {
                            p.max_height(height)
                        }
                    })
                    .gap(0.)
                    .on_dismiss(move |window, cx| {
                        let _ = weak.update(cx, |this, cx| this.dismiss_details(window, cx));
                    })
                    .child(
                        div()
                            .id("todo-details-dialog")
                            .debug_selector(|| "todo-details-dialog".into())
                            .role(Role::Dialog)
                            .aria_label("To-do details")
                            .track_focus(&self.detail_focus)
                            .w_full()
                            .when(editing, |d| d.h_full())
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .on_key_down(cx.listener(
                                |this, event: &gpui::KeyDownEvent, window, cx| {
                                    if event.keystroke.key == "escape" {
                                        this.dismiss_details(window, cx);
                                        cx.stop_propagation();
                                    }
                                },
                            ))
                            .children(error.map(|error| {
                                div()
                                    .px_3()
                                    .py_2()
                                    .text_size(px(page::META_SIZE))
                                    .text_color(vorkspace_ui::theme::danger())
                                    .child(error.clone())
                            }))
                            .when(
                                self.saved_details_key(item, Some(&editor.id)).is_some(),
                                |d| {
                                    d.child(
                                        div().px_3().py_2().flex_shrink_0().child(
                                            labeled_secondary(
                                                "todo-switch-draft",
                                                "Keep draft and switch",
                                                false,
                                                self.pending_changes > 0,
                                            )
                                            .debug_selector(|| "todo-switch-draft".into())
                                            .on_click(
                                                cx.listener(|this, _, window, cx| {
                                                    this.switch_details_draft(window, cx);
                                                }),
                                            ),
                                        ),
                                    )
                                },
                            )
                            .child(body),
                    ),
                )
                .into_any_element(),
        )
    }

    fn details_preview(
        &self,
        item: &TodoItem,
        editor: &DetailsEditor,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let id = item.id.clone();
        let status = item.status;
        let pending = item.sync == vorkspace_core::todos::TodoSync::Pending
            || item.sync == vorkspace_core::todos::TodoSync::Uncertain;
        div()
            .id("todo-details")
            .debug_selector(|| "todo-details".into())
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(52.))
                    .px_4()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(Self::status_mark(status))
                    .child(page::meta().flex_1().child(match &item.issue {
                        Some(issue) => format!("{} · #{}", status.label(), issue.number),
                        None => status.label().into(),
                    }))
                    .when_some(item.issue.clone(), |d, issue| {
                        d.child(
                            page::icon_action("todo-open-issue", "Open on GitHub", true)
                                .child(icon("external-link", page::ICON_SIZE))
                                .on_click(move |_, _, cx| cx.open_url(&issue.url)),
                        )
                    })
                    .child(
                        page::icon_action("todo-details-close", "Close details", true)
                            .debug_selector(|| "todo-details-close".into())
                            .child(icon("close", page::ICON_SIZE))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.dismiss_details(window, cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .id("todo-details-scroll")
                    .debug_selector(|| "todo-details-scroll".into())
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_4()
                    .pb_4()
                    .child(
                        page::title()
                            .mb_4()
                            .child(editor.title.read(cx).content.clone()),
                    )
                    .child(editor.preview.clone())
                    .when(!item.attachments.is_empty(), |d| {
                        d.child(
                            div()
                                .mt_5()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(page::label().child("Attachments"))
                                .children(item.attachments.iter().enumerate().map(
                                    |(index, reference)| {
                                        div()
                                            .id(("todo-attachment", index))
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(icon("bot-attach", 14.))
                                            .child(page::meta().child(reference.clone()))
                                            .when(reference.starts_with("https://"), |d| {
                                                let url = reference.clone();
                                                d.cursor_pointer()
                                                    .hover(|s| s.text_color(accent_color()))
                                                    .on_click(move |_, _, cx| cx.open_url(&url))
                                            })
                                    },
                                )),
                        )
                    })
                    .when(!item.dependencies.is_empty(), |d| {
                        d.child(
                            div()
                                .mt_5()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(page::label().child("Depends on"))
                                .children(item.dependencies.iter().enumerate().map(
                                    |(index, reference)| {
                                        let target = self.dependency_item(item, reference).cloned();
                                        div()
                                            .id(("todo-detail-dependency", index))
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(icon("git-branch", 14.))
                                            .child(
                                                page::meta()
                                                    .child(self.dependency_label(item, reference)),
                                            )
                                            .when_some(target, |d, target| {
                                                d.cursor_pointer()
                                                    .hover(|s| s.text_color(accent_color()))
                                                    .on_click(cx.listener(
                                                        move |this, _, window, cx| {
                                                            this.present_details(
                                                                &target, window, cx,
                                                            )
                                                        },
                                                    ))
                                            })
                                    },
                                )),
                        )
                    }),
            )
            .child(
                div()
                    .id("todo-details-footer")
                    .debug_selector(|| "todo-details-footer".into())
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(page::divider())
                    .p_3()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        labeled_primary("todo-details-edit", "Edit", false)
                            .debug_selector(|| "todo-details-edit".into())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.detail_editing = true;
                                cx.notify();
                            })),
                    )
                    .child(
                        labeled_secondary(
                            "todo-status",
                            format!("Move to {}", status.next().label()),
                            false,
                            pending || self.pending_changes > 0,
                        )
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.advance(id.clone(), status, cx)),
                        ),
                    )
                    .child(
                        labeled_secondary(
                            "todo-delete",
                            if item.issue.is_some() {
                                "Close issue"
                            } else {
                                "Delete"
                            },
                            true,
                            pending || self.pending_changes > 0,
                        )
                        // The destructive action stands apart from the ones
                        // that move the work forward.
                        .ml_auto()
                        .on_click(cx.listener({
                            let id = item.id.clone();
                            move |this, _, window, cx| {
                                this.dismiss_details(window, cx);
                                this.delete(id.clone(), cx);
                            }
                        })),
                    ),
            )
            .into_any_element()
    }
}
