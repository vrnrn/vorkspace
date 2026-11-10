//! Library and document layouts, with their native interactions.
use super::*;

impl Notes {
    fn project_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let query = self.project_query.read(cx).content.to_lowercase();
        let mut projects = self.projects.clone();
        if notes::is_project(&self.cwd) {
            projects.insert(0, self.cwd.clone());
        }
        for note in &self.notes {
            if let Some(path) = note
                .source_cwd
                .as_ref()
                .filter(|path| notes::is_project(path))
            {
                projects.push(path.clone());
            }
        }
        let mut seen = std::collections::HashSet::new();
        projects.retain(|path| seen.insert(path.clone()) && path.to_lowercase().contains(&query));
        let weak = cx.weak_entity();
        popover(
            "note-project-picker",
            Anchor::Slot(self.project_anchor.clone()),
        )
        .width(300.)
        .max_height(340.)
        .on_dismiss(move |_, cx| {
            let _ = weak.update(cx, |this, cx| {
                this.project_open = false;
                cx.notify();
            });
        })
        .child(
            div()
                .p_2()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().px_2().py_1().child(self.project_query.clone()))
                .child(
                    div()
                        .id("note-no-project")
                        .role(Role::Button)
                        .map(vorkspace_ui::theme::tab_reachable)
                        .aria_label("No project")
                        .px_2()
                        .py_1p5()
                        .rounded(px(radius::CONTAINER))
                        .text_size(px(12.))
                        .hover(|style| style.bg(vorkspace_ui::fluid::hover(0.08)))
                        .child("No project")
                        .on_click(cx.listener(|this, _, _, cx| this.move_project("~".into(), cx))),
                )
                .child(
                    div()
                        .id("note-project-results")
                        .min_h_0()
                        .overflow_y_scroll()
                        .children(projects.into_iter().enumerate().map(|(ix, path)| {
                            let name = project_name(&path);
                            div()
                                .id(("note-project", ix))
                                .role(Role::Button)
                                .aria_label(path.clone())
                                .px_2()
                                .py_1p5()
                                .rounded(px(radius::CONTAINER))
                                .text_size(px(12.))
                                .hover(|style| style.bg(vorkspace_ui::fluid::hover(0.08)))
                                .child(name)
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(page::META_SIZE))
                                        .text_color(page::secondary_ink())
                                        .child(path.clone()),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.move_project(path.clone(), cx)
                                }))
                        })),
                ),
        )
        .into_any_element()
    }

    fn shareable_note(&self, cx: &App) -> Option<Note> {
        let mut note = self.draft(cx)?;
        if self.show_recording {
            let recording = self.recording.read(cx);
            if !recording.is_for_note(&note.id) {
                return None;
            }
            note.body = recording.selected_text()?.to_owned();
        }
        (!note.body.trim().is_empty()).then_some(note)
    }

    fn detail(&mut self, note: Note, narrow: bool, cx: &mut Context<Self>) -> AnyElement {
        let source = note.source_session_id.clone();
        let attached = self.recordings.contains_key(&note.id);
        let recording_ready = self.recording.read(cx).is_for_note(&note.id);
        let can_add = if self.show_recording {
            recording_ready
                && self
                    .recording
                    .read(cx)
                    .selected_text()
                    .is_some_and(|text| !text.trim().is_empty())
        } else {
            !note.body.trim().is_empty()
        };
        let can_delete = self.image_note.as_ref() != Some(&note.id)
            && (!attached || (recording_ready && self.recording.read(cx).can_delete_note()));
        let open_tab = self.pinned.is_none();
        let tab_note_id = note.id.clone();
        let project = page::action("note-move-project", "Move note to project", true)
            .relative()
            .min_w_0()
            .max_w_full()
            .child(self.project_anchor.probe())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        note.source_cwd
                            .as_deref()
                            .map(|path| self.project_mark(path, page::ICON_SIZE))
                            .unwrap_or_else(|| icon("folder", page::ICON_SIZE).into_any_element()),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .child(note.project_name().unwrap_or_else(|| "No project".into())),
                    )
                    .child(icon("chevron-down", page::ICON_SIZE)),
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.project_open = !this.project_open;
                if this.project_open {
                    this.project_query
                        .update(cx, |input, cx| input.set_text(String::new(), cx));
                    window.focus(&this.project_query.focus_handle(cx), cx);
                }
                cx.notify();
            }));
        let tags = page::meta()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .child("Tags")
            .children(note.tags.iter().enumerate().map(|(ix, tag)| {
                let tag = tag.clone();
                div()
                    .h(px(page::CONTROL_HEIGHT))
                    .max_w(px(192.))
                    .flex()
                    .items_center()
                    .gap_1()
                    .rounded(px(radius::CONTROL))
                    .bg(content(0.08))
                    .pl_2()
                    .pr_1()
                    .text_color(content(0.7))
                    .child(div().truncate().child(format!("#{tag}")))
                    .child(
                        page::icon_action(("remove-note-tag", ix), format!("Remove #{tag}"), true)
                            .child(icon("close", page::ICON_SIZE))
                            .on_click(cx.listener(move |this, _, _, cx| this.remove_tag(&tag, cx))),
                    )
            }))
            .when(note.tags.len() < notes::MAX_TAGS, |d| {
                d.child(page::field_frame().w(px(180.)).child(self.tag.clone()))
            });
        let actions = div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap_2()
            .child(
                page::primary("note-add-to-chat", "Add to chat", !can_add)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if can_add {
                            this.flush(cx);
                            if !this.dirty
                                && let Some(note) = this.shareable_note(cx)
                            {
                                cx.emit(Event::AddToChat(note));
                            }
                        }
                    }))
                    .child("Add to chat"),
            )
            .when_some(source, |d, source| {
                d.child(
                    page::action("note-source-session", "Open source conversation", true)
                        .child("Source conversation")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.flush(cx);
                            if this.dirty {
                                return;
                            }
                            if let Some(id) = locked(&this.store)
                                .ok()
                                .and_then(|store| store.resolve_existing_session_id(&source))
                            {
                                cx.emit(Event::OpenSource(id));
                            } else {
                                this.error = Some("Source conversation no longer exists.".into());
                                cx.notify();
                            }
                        })),
                )
            })
            .child(
                page::icon_action("copy-note", "Copy selected note text", can_add)
                    .child(icon("response-copy", page::ICON_SIZE))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(note) = this.shareable_note(cx) {
                            cx.write_to_clipboard(ClipboardItem::new_string(note.body));
                        }
                    })),
            )
            // Deleting is rare and destructive, so it waits at the far end as
            // a quiet icon and only grows into words once it asks to confirm.
            .child(
                div()
                    .ml_auto()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(self.confirm_delete, |d| {
                        d.child(
                            page::action("cancel-delete-note", "Cancel deletion", true)
                                .child("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.confirm_delete = false;
                                    cx.notify();
                                })),
                        )
                        .child(
                            page::secondary("delete-note", "Delete note", true, !can_delete)
                                .child(icon("trash", page::ICON_SIZE))
                                .child("Confirm delete")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if can_delete {
                                        this.delete(cx);
                                    }
                                })),
                        )
                    })
                    .when(!self.confirm_delete, |d| {
                        d.child(
                            page::icon_action("delete-note", "Delete note", can_delete)
                                .child(icon("trash", page::ICON_SIZE))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if can_delete {
                                        this.delete(cx);
                                    }
                                })),
                        )
                    }),
            );
        let actions = if recording_ready {
            self.recording.update(cx, |page, cx| {
                page.note_controls(actions.into_any_element(), self.show_recording, cx)
            })
        } else {
            actions.into_any_element()
        };
        let tabs = div()
            .id("note-sections")
            .role(Role::TabList)
            .aria_label("Note sections")
            .h(px(page::TOOLBAR_HEIGHT))
            .flex()
            .items_center()
            .gap_1()
            .border_b_1()
            .border_color(page::divider())
            .children(
                [(false, "Preview"), (true, "Source")].map(|(source, label)| {
                    page::tab(
                        if source {
                            "note-source"
                        } else {
                            "note-preview"
                        },
                        label,
                        !self.show_recording && self.source == source,
                    )
                    .child(label)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.source = source;
                        this.show_recording = false;
                        this.recording
                            .update(cx, |page, cx| page.hide_note_content(cx));
                        if let Some(id) = &this.selected {
                            this.modes.insert(id.clone(), source);
                        }
                        if source {
                            window.focus(&this.body.focus_handle(cx), cx);
                        } else {
                            this.flush(cx);
                            this.update_preview(cx);
                        }
                        cx.notify();
                    }))
                }),
            )
            .when(attached, |d| {
                d.children(
                    [(false, "Summary"), (true, "Transcript")].map(|(transcript, label)| {
                        page::tab(
                            if transcript {
                                "note-transcript"
                            } else {
                                "note-summary"
                            },
                            label,
                            self.show_recording
                                && self.recording.read(cx).transcript_selected() == transcript,
                        )
                        .child(label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.flush(cx);
                            if this.dirty {
                                return;
                            }
                            this.show_recording = true;
                            this.recording
                                .update(cx, |page, cx| page.note_section(transcript, cx));
                            cx.notify();
                        }))
                    }),
                )
            });
        let editor = div()
            .id("note-image-drop-zone")
            .relative()
            .min_h(px(448.))
            .rounded(px(radius::CONTAINER))
            .border_1()
            .border_color(if self.image_drag {
                accent_color()
            } else {
                content(0.)
            })
            .on_drag_move::<ExternalPaths>(cx.listener(
                |this, event: &gpui::DragMoveEvent<ExternalPaths>, _, cx| {
                    this.image_drag = event.bounds.contains(&event.event.position);
                    cx.notify();
                },
            ))
            .on_drag_move::<DraggedFile>(cx.listener(
                |this, event: &gpui::DragMoveEvent<DraggedFile>, _, cx| {
                    this.image_drag = event.bounds.contains(&event.event.position);
                    cx.notify();
                },
            ))
            .on_file_drop_exit(cx.listener(|this, _, _, cx| {
                this.image_drag = false;
                cx.notify();
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.add_images(paths.paths().to_vec(), cx)
            }))
            .on_drop(cx.listener(|this, file: &DraggedFile, _, cx| {
                this.add_images(vec![file.0.clone()], cx)
            }))
            .when(self.source, |d| {
                d.h(px((self.body.read(cx).editor_line_count() as f32
                    * page::BODY_LINE
                    + 16.)
                    .clamp(448., 2000.)))
                    .child(self.body.clone())
            })
            .when(!self.source, |d| {
                d.when_some(self.document.clone(), |d, preview| d.child(preview))
            })
            .when(self.image_drag || self.image_note.is_some(), |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(px(radius::CONTAINER))
                        .bg(pane_fill().opacity(0.9))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.))
                        .text_color(content(0.7))
                        .child(if self.image_note.is_some() {
                            "Adding images…"
                        } else {
                            "Drop images here"
                        }),
                )
            });
        div()
            .id("note-detail")
            .debug_selector(|| "note-detail".into())
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.detail_scroll)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .gap(px(16.))
                    .w_full()
                    .max_w(px(page::DETAIL_MAX))
                    .mx_auto()
                    .p(page::detail_padding(narrow))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                page::meta()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap_2()
                                    .child(icon("note", page::ICON_SIZE))
                                    .child("Note")
                                    .child(div().min_w_0().truncate().child(note.slug.clone()))
                                    .child(project)
                                    .when(open_tab, |d| {
                                        d.child(
                                            page::icon_action(
                                                "open-note-tab",
                                                "Open note in tab",
                                                true,
                                            )
                                            .debug_selector(|| "open-note-tab".into())
                                            .child(icon("external-link", page::ICON_SIZE))
                                            .on_click(
                                                cx.listener(move |this, _, _, cx| {
                                                    this.request_open_tab(&tab_note_id, cx)
                                                }),
                                            ),
                                        )
                                    }),
                            )
                            .child(page::title().child(self.title.clone()))
                            .child(
                                page::meta()
                                    .child(format!("Updated {}", relative_time(note.updated_at))),
                            )
                            .child(tags)
                            .child(actions),
                    )
                    .child(tabs)
                    .child(if self.show_recording && recording_ready {
                        self.recording.read(cx).note_content()
                    } else {
                        editor.into_any_element()
                    }),
            )
            .into_any_element()
    }
}

impl gpui::Render for Notes {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.blurs_bound {
            self.blurs_bound = true;
            self._subscriptions.push(cx.on_blur(
                &self.title.focus_handle(cx),
                window,
                |this, _, cx| this.flush(cx),
            ));
            self._subscriptions.push(cx.on_blur(
                &self.tag.focus_handle(cx),
                window,
                |this, _, cx| this.add_tag(cx),
            ));
        }
        if !cx.has_active_drag() {
            self.image_drag = false;
        }
        let query = self.query.read(cx).content.to_string();
        let visible: Vec<_> = self
            .notes
            .iter()
            .filter(|note| note.matches(&query))
            .cloned()
            .collect();
        let narrow = self.is_narrow(window);
        let show_list =
            self.pinned.is_none() && (!narrow || self.show_list || self.selected.is_none());
        let show_detail = self.pinned.is_some() || !narrow || !show_list;
        if show_detail
            && self.show_recording
            && self
                .selected
                .as_deref()
                .is_some_and(|id| self.recording.read(cx).is_for_note(id))
        {
            self.recording
                .update(cx, |page, cx| page.focus_pending(window, cx));
        }
        let list = div()
            .relative()
            .w(px(self.list_width))
            .when(narrow, |d| d.w_full())
            .h_full()
            .flex_shrink_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                page::toolbar()
                    .child(
                        page::search_frame(&self.query.focus_handle(cx))
                            .flex_1()
                            .child(icon("search", page::ICON_SIZE))
                            .child(self.query.clone()),
                    )
                    .child(
                        page::icon_action("create-note", "New note", true)
                            .debug_selector(|| "create-note".into())
                            .child(icon("plus", page::ICON_SIZE))
                            .on_click(cx.listener(|this, _, window, cx| this.create(window, cx))),
                    ),
            )
            .child(
                div()
                    .id("notes-list")
                    .debug_selector(|| "notes-list".into())
                    .role(Role::ListBox)
                    .aria_label("Saved notes")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.list_scroll)
                    .p(px(4.))
                    .when(visible.is_empty(), |d| {
                        d.child(page::meta().p(px(12.)).child(if self.loading {
                            "Loading notes…"
                        } else if !query.trim().is_empty() {
                            "No matching notes"
                        } else {
                            "No notes yet. Save a turn from the transcript, or create one here."
                        }))
                    })
                    .children(
                        {
                            // One shared project tells the reader nothing per row.
                            let one_project = visible
                                .windows(2)
                                .all(|w| w[0].source_cwd == w[1].source_cwd);
                            visible
                                .iter()
                                .enumerate()
                                .map(move |(ix, note)| (ix, note, one_project))
                        }
                        .map(|(ix, note, one_project)| {
                            let id = note.id.clone();
                            let active = self.selected.as_ref() == Some(&id);
                            let preview = note.preview();
                            page::row(("saved-note", ix), note.title.clone(), active)
                                .aria_selected(active)
                                .mb(px(2.))
                                .when(!one_project, |d| {
                                    d.child(
                                        page::meta()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(div().flex_1().truncate().when_some(
                                                note.project_name(),
                                                |d, name| {
                                                    d.child(
                                                        div()
                                                            .flex()
                                                            .items_center()
                                                            .gap_1p5()
                                                            .child(
                                                                self.project_mark(
                                                                    note.source_cwd
                                                                        .as_deref()
                                                                        .unwrap_or(""),
                                                                    12.,
                                                                ),
                                                            )
                                                            .child(name),
                                                    )
                                                },
                                            ))
                                            .child(relative_time(note.updated_at)),
                                    )
                                })
                                .child(
                                    page::label()
                                        .when(!one_project, |d| d.mt_1())
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .when(self.recordings.contains_key(&note.id), |d| {
                                            d.child(icon("mic", page::ICON_SIZE))
                                        })
                                        .child(div().flex_1().min_w_0().truncate().child(
                                            if note.title.is_empty() {
                                                "Untitled".into()
                                            } else {
                                                note.title.clone()
                                            },
                                        ))
                                        .when(one_project, |d| {
                                            d.child(
                                                page::meta()
                                                    .flex_shrink_0()
                                                    .font_weight(FontWeight::NORMAL)
                                                    .child(relative_time(note.updated_at)),
                                            )
                                        }),
                                )
                                .when(!preview.is_empty(), |d| {
                                    d.child(page::meta().mt_1().truncate().child(preview))
                                })
                                .when(!note.tags.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .mt_1p5()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .overflow_hidden()
                                            .children(note.tags.iter().take(3).map(|tag| {
                                                div()
                                                    .max_w(px(96.))
                                                    .truncate()
                                                    .rounded(px(radius::SMALL))
                                                    .bg(content(0.08))
                                                    .px_1p5()
                                                    .py(px(2.))
                                                    .text_size(px(page::META_SIZE))
                                                    .text_color(page::secondary_ink())
                                                    .child(format!("#{tag}"))
                                            }))
                                            .when(note.tags.len() > 3, |d| {
                                                d.child(
                                                    div()
                                                        .text_size(px(page::META_SIZE))
                                                        .text_color(page::secondary_ink())
                                                        .child(format!("+{}", note.tags.len() - 3)),
                                                )
                                            }),
                                    )
                                })
                                .on_click(cx.listener({
                                    let id = id.clone();
                                    move |this, _, _, cx| this.select(id.clone(), cx)
                                }))
                                .on_mouse_down(
                                    MouseButton::Middle,
                                    cx.listener(move |this, _, _, cx| {
                                        this.request_open_tab(&id, cx);
                                        cx.stop_propagation();
                                    }),
                                )
                        }),
                    ),
            )
            .when(!narrow, |d| {
                d.child(
                    page::detail_resize_handle(
                        "resize-notes-list",
                        "Resize notes list",
                        self.resize.is_some(),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                            if event.click_count == 2 {
                                this.list_width = page::LIST_WIDTH;
                            } else {
                                this.resize = Some((event.position.x.as_f32(), this.list_width));
                            }
                            cx.notify();
                        }),
                    ),
                )
            });
        let detail = if let Some(note) = self.draft(cx) {
            self.detail(note, narrow, cx)
        } else {
            page::empty(
                icon("note", 24.),
                if self.pinned.is_some() {
                    "Note unavailable"
                } else {
                    "Select a note"
                },
                if self.pinned.is_some() {
                    "This note was deleted or is not available."
                } else {
                    "Choose a saved note, or create one with the + button."
                },
            )
            .flex_1()
            .into_any_element()
        };
        div()
            .id("notes-view")
            .track_focus(&self.focus)
            .role(Role::Region)
            .aria_label("Notes")
            .size_full()
            .flex()
            .flex_col()
            .text_color(content(1.))
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                let key = event.keystroke.key.as_str();
                if key == "enter"
                    && this.title.focus_handle(cx).is_focused(window)
                    && !this.title.read(cx).is_composing()
                {
                    this.flush(cx);
                    window.focus(&this.query.focus_handle(cx), cx);
                    cx.stop_propagation();
                } else if key == "escape" {
                    if this.project_open {
                        this.project_open = false;
                    } else if this.pinned.is_none()
                        && this.is_narrow(window)
                        && !this.show_list
                        && this.selected.is_some()
                    {
                        this.show_list = true;
                        window.focus(&this.query.focus_handle(cx), cx);
                    } else {
                        this.flush(cx);
                        if !this.dirty {
                            cx.emit(Event::Close);
                        }
                    }
                    cx.stop_propagation();
                    cx.notify();
                } else if (event.keystroke.modifiers.platform || event.keystroke.modifiers.control)
                    && key == "s"
                {
                    this.flush(cx);
                    cx.stop_propagation();
                } else if (event.keystroke.modifiers.platform || event.keystroke.modifiers.control)
                    && key == "n"
                    && this.pinned.is_none()
                {
                    this.create(window, cx);
                    cx.stop_propagation();
                } else if key == "backspace"
                    && this.tag.focus_handle(cx).is_focused(window)
                    && this.tag.read(cx).content.is_empty()
                    && let Some(tag) = this.draft(cx).and_then(|note| note.tags.last().cloned())
                {
                    this.remove_tag(&tag, cx);
                }
            }))
            .on_mouse_move(
                cx.listener(|this, event: &gpui::MouseMoveEvent, window, cx| {
                    if let Some((start, width)) = this.resize {
                        this.list_width = (width + event.position.x.as_f32() - start).clamp(
                            page::LIST_MIN,
                            page::LIST_MAX
                                .min(this.available_width(window) * 0.5)
                                .max(page::LIST_MIN),
                        );
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.resize = None),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.resize = None),
            )
            .when(narrow && show_detail && self.pinned.is_none(), |d| {
                d.child(
                    page::toolbar().child(
                        page::action("notes-back", "Back to notes", true)
                            .debug_selector(|| "notes-back".into())
                            .child(icon("chevron-left", page::ICON_SIZE))
                            .child("Notes")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_list = true;
                                window.focus(&this.query.focus_handle(cx), cx);
                                cx.notify();
                            })),
                    ),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .flex()
                    .when(show_list, |d| d.child(list))
                    .when(show_detail, |d| {
                        d.child(page::detail_pane(!narrow && show_list).child(detail))
                    }),
            )
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    div()
                        .id("note-error")
                        .role(Role::Alert)
                        .flex_shrink_0()
                        .px_4()
                        .py_2()
                        .flex()
                        .when(self.save_conflict, |d| d.flex_wrap())
                        .items_center()
                        .gap_2()
                        .text_size(px(12.))
                        .text_color(vorkspace_ui::theme::danger())
                        .child(div().flex_1().child(error))
                        .when(!self.save_conflict, |d| {
                            d.child(
                                div()
                                    .id("retry-note-save")
                                    .role(Role::Button)
                                    .map(vorkspace_ui::theme::tab_reachable)
                                    .aria_label("Retry saving note")
                                    .underline()
                                    .child("Retry")
                                    .on_click(cx.listener(|this, _, _, cx| this.flush(cx))),
                            )
                        })
                        .when(self.save_conflict, |d| {
                            d.child(
                                page::secondary("note-save-copy", "Save a Copy", false, false)
                                    .debug_selector(|| "note-save-copy".into())
                                    .child("Save a Copy")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.resolve_save_conflict(true, cx)
                                    })),
                            )
                            .child(
                                page::secondary(
                                    "note-reload-latest",
                                    "Discard and Reload",
                                    false,
                                    false,
                                )
                                .debug_selector(|| "note-reload-latest".into())
                                .child("Discard and Reload")
                                .on_click(cx.listener(
                                    |this, _, _, cx| this.resolve_save_conflict(false, cx),
                                )),
                            )
                        }),
                )
            })
            .when(self.project_open, |d| d.child(self.project_picker(cx)))
    }
}

fn relative_time(at: i64) -> String {
    let seconds = (vorkspace_core::time::now_ms() as i64 - at).max(0) / 1000;
    match seconds {
        0..60 => "now".into(),
        60..3600 => format!("{}m", seconds / 60),
        3600..86400 => format!("{}h", seconds / 3600),
        86400..604800 => format!("{}d", seconds / 86400),
        _ => chrono::DateTime::from_timestamp_millis(at)
            .map(|date| date.format("%b %-d").to_string())
            .unwrap_or_default(),
    }
}
