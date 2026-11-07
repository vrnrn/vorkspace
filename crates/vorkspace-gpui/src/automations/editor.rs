use super::*;
use chrono::{Local, TimeZone};
use gpui::AnimationExt;
use vorkspace_ui::radius;

pub(super) fn toggle(id: impl Into<gpui::ElementId>, on: bool) -> gpui::Stateful<gpui::Div> {
    crate::controls::compact_switch(
        id,
        if on {
            "Pause automation"
        } else {
            "Enable automation"
        },
        on,
    )
}
fn options(values: &[(&str, &str)]) -> Vec<(String, String)> {
    values
        .iter()
        .map(|(v, l)| (v.to_string(), l.to_string()))
        .collect()
}
pub(super) fn events(kind: &str) -> Vec<(String, String)> {
    options(match kind {
        "time" => &[
            ("hourly", "Hourly"),
            ("daily", "Daily"),
            ("weekdays", "Weekdays"),
            ("weekly", "Weekly"),
        ],
        "github" => &[
            ("draft_opened", "Draft opened"),
            ("pull_request_opened", "Pull request opened"),
            ("issue_opened", "Issue opened"),
        ],
        "gitlab" => &[
            ("merge_request_opened", "Merge request opened"),
            ("issue_opened", "Issue opened"),
        ],
        "jira" => &[("issue_created", "Issue appeared")],
        _ => &[],
    })
}
fn section(label: &'static str) -> gpui::Div {
    page::label()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(label)
}
fn settings_row(label: &'static str, hint: &'static str, control: impl IntoElement) -> gpui::Div {
    div()
        .px(px(12.))
        .py(px(8.))
        .border_b_1()
        .border_color(page::divider())
        .flex()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap(px(8.))
        .child(
            div()
                .flex_grow(1.)
                .flex_basis(px(192.))
                .min_w_0()
                .flex()
                .flex_col()
                .child(page::label().child(label))
                .child(page::meta().child(hint)),
        )
        .child(control)
}
impl Automations {
    fn select_control(
        &self,
        id: impl Into<gpui::ElementId>,
        label: String,
        field: Choice,
        values: Vec<(String, String)>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let anchor = AnchorSlot::default();
        let captured = anchor.clone();
        let enabled = !matches!(field, Choice::Conversation)
            || self
                .editor
                .as_ref()
                .is_none_or(|editor| editor.draft.workspace_mode != "worktree");
        page::secondary(id, label.clone(), false, !enabled)
            .relative()
            .min_w_0()
            .max_w_full()
            .flex_shrink_1()
            .child(anchor.probe())
            .when(matches!(field, Choice::Access), |d| {
                let mode = self
                    .editor
                    .as_ref()
                    .map(|e| e.draft.runtime_mode.as_str())
                    .unwrap_or("auto");
                d.child(icon(
                    match mode {
                        "supervised" => "lock",
                        "auto-accept-edits" => "pencil",
                        "full-access" => "shield",
                        _ => "sparkles",
                    },
                    page::ICON_SIZE,
                ))
            })
            .child(div().min_w_0().truncate().child(label))
            .child(icon("chevron-down", page::ICON_SIZE).text_color(page::secondary_ink()))
            .on_click(cx.listener(move |this, _, _, cx| {
                if matches!(field, Choice::Conversation)
                    && this
                        .editor
                        .as_ref()
                        .is_some_and(|e| e.draft.workspace_mode == "worktree")
                {
                    return;
                }
                this.choice = Some(ChoiceMenu {
                    field: field.clone(),
                    options: values.clone(),
                    anchor: captured.clone(),
                });
                this.trigger_menu = None;
                this.models = None;
                this.project_picker = None;
                cx.notify();
            }))
    }
    pub(super) fn apply_choice(&mut self, field: Choice, value: String, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        match field {
            Choice::Access => editor.draft.runtime_mode = value,
            Choice::Workspace => {
                editor.draft.workspace_mode = value;
                editor.draft.worktree_cwd.clear();
                if editor.draft.workspace_mode == "worktree" {
                    editor.draft.reuse_session = false;
                }
            }
            Choice::Conversation => editor.draft.reuse_session = value == "reuse",
            Choice::Folder => editor.draft.session_folder_id = value,
            Choice::Grace => editor.draft.missed_run_grace_minutes = value.parse().unwrap_or(720),
            Choice::ModelSetting(key) => {
                set_control_value(&mut editor.draft, key, value);
            }
            Choice::AddTrigger => {
                if let Some((kind, event)) = value.split_once(':') {
                    let triggers = editor.draft.triggers.get_or_insert_with(Vec::new);
                    if triggers.len() < 20 {
                        triggers.push(AutomationTrigger::new(kind, event));
                    }
                }
            }
            Choice::TriggerEvent(id) | Choice::Schedule(id) => {
                if let Some(trigger) = editor
                    .draft
                    .triggers
                    .as_mut()
                    .and_then(|ts| ts.iter_mut().find(|t| t.id == id))
                {
                    trigger.event = value.clone();
                    if trigger.kind == "time" {
                        trigger.schedule_kind = value;
                    }
                }
            }
            Choice::Minute(id) => {
                if let Some(trigger) = editor
                    .draft
                    .triggers
                    .as_mut()
                    .and_then(|ts| ts.iter_mut().find(|t| t.id == id))
                {
                    trigger.minute = value.parse().unwrap_or(0);
                }
            }
            Choice::Time(id) => {
                if let Some(trigger) = editor
                    .draft
                    .triggers
                    .as_mut()
                    .and_then(|ts| ts.iter_mut().find(|t| t.id == id))
                {
                    trigger.time = value.clone();
                }
                if let Some(fields) = editor.fields.iter().find(|f| f.id == id) {
                    fields
                        .time
                        .update(cx, |input, cx| input.set_text(value, cx));
                }
            }
            Choice::Weekday(id) => {
                if let Some(trigger) = editor
                    .draft
                    .triggers
                    .as_mut()
                    .and_then(|ts| ts.iter_mut().find(|t| t.id == id))
                {
                    trigger.day_of_week = value.parse().unwrap_or(1);
                }
            }
        }
        editor.dirty = true;
        self.choice = None;
        self.rebuild_trigger_fields(cx);
        cx.notify();
    }
    pub(super) fn render_model_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(editor) = &self.editor else {
            return div().into_any_element();
        };
        if cx
            .try_global::<vorkspace_ui::preferences::Preferences>()
            .is_some_and(|p| p.model_controls == "beside")
            || self.model_controls.is_empty()
        {
            return div().into_any_element();
        }
        page::meta()
            .p(px(8.))
            .border_b_1()
            .border_color(page::divider())
            .flex()
            .flex_col()
            .gap(px(4.))
            .children(
                self.model_controls
                    .iter()
                    .enumerate()
                    .map(|(index, control)| {
                        page::meta()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .justify_between()
                            .gap(px(8.))
                            .child(control.name.clone())
                            .child(self.select_control(
                                ("automation-model-menu-setting", index),
                                control_label(control, &editor.draft),
                                Choice::ModelSetting(control.id.clone()),
                                control.choices.clone(),
                                cx,
                            ))
                    }),
            )
            .into_any_element()
    }
    fn render_trigger(
        &self,
        trigger: &AutomationTrigger,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let fields = self
            .editor
            .as_ref()
            .and_then(|e| e.fields.iter().find(|f| f.id == trigger.id));
        let id = trigger.id.clone();
        let mut sentence = page::label()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(8.))
            .gap_y(px(4.))
            .text_color(page::secondary_ink());
        if trigger.kind == "time" {
            let prefix = match trigger.schedule_kind.as_str() {
                "hourly" => "Every hour at",
                "daily" => "Every day at",
                "weekly" => "Every week on",
                _ => "Every weekday at",
            };
            let frequency_anchor = AnchorSlot::default();
            let captured = frequency_anchor.clone();
            let schedule_id = id.clone();
            // The schedule reads as one sentence. Keep frequency editable
            // without adding an extra pill to the sentence.
            sentence = sentence.child(
                page::action(
                    ("trigger-frequency", index),
                    format!("Schedule: {prefix}"),
                    true,
                )
                .relative()
                .child(frequency_anchor.probe())
                .child(prefix)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.choice = Some(ChoiceMenu {
                        field: Choice::Schedule(schedule_id.clone()),
                        options: events("time"),
                        anchor: captured.clone(),
                    });
                    cx.notify();
                })),
            );
            if trigger.schedule_kind == "weekly" {
                sentence = sentence
                    .child(
                        self.select_control(
                            ("trigger-day", index),
                            automations::WEEKDAYS
                                .get(trigger.day_of_week as usize)
                                .unwrap_or(&"Monday")
                                .to_string(),
                            Choice::Weekday(id.clone()),
                            automations::WEEKDAYS
                                .iter()
                                .enumerate()
                                .map(|(i, s)| (i.to_string(), s.to_string()))
                                .collect(),
                            cx,
                        ),
                    )
                    .child("at");
            }
            let mut preview_trigger = trigger.clone();
            if let Some(fields) = fields {
                preview_trigger.time = fields.time.read(cx).content.to_string();
            }
            if trigger.schedule_kind == "hourly" {
                sentence = sentence.child(
                    self.select_control(
                        ("trigger-minute", index),
                        format!(":{:02}", trigger.minute),
                        Choice::Minute(id.clone()),
                        (0..60)
                            .map(|m| (m.to_string(), format!(":{m:02}")))
                            .collect(),
                        cx,
                    ),
                );
            } else {
                let time = preview_trigger.time.clone();
                let mut times = (0..24)
                    .flat_map(|hour| {
                        [0, 30].map(move |minute| {
                            let value = format!("{hour:02}:{minute:02}");
                            (value.clone(), value)
                        })
                    })
                    .collect::<Vec<_>>();
                if !times.iter().any(|(value, _)| value == &time) {
                    times.insert(0, (time.clone(), time.clone()));
                }
                sentence = sentence.child(self.select_control(
                    ("trigger-time", index),
                    time,
                    Choice::Time(id.clone()),
                    times,
                    cx,
                ));
            }
            sentence = sentence
                .child(page::meta().child(gmt_offset()))
                .child(page::meta().child(next_run_preview(
                    preview_trigger.next_at(vorkspace_core::time::now_ms() as i64),
                )));
        } else {
            sentence = sentence
                .child(self.select_control(
                    ("trigger-event", index),
                    automations::event_label(&trigger.kind, &trigger.event),
                    Choice::TriggerEvent(id.clone()),
                    events(&trigger.kind),
                    cx,
                ))
                .child("by");
            if let Some(fields) = fields {
                sentence = sentence.child(
                    div()
                        .w(px(80.))
                        .h(px(page::CONTROL_HEIGHT))
                        .rounded(px(radius::CONTROL))
                        .bg(content(0.1))
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .child(fields.actor.clone()),
                );
            }
        }
        let row = div()
            .min_h(px(page::TOOLBAR_HEIGHT))
            .py(px(4.))
            .flex()
            .items_center()
            .gap(px(8.))
            .child(trigger_mark(&trigger.kind, page::ICON_SIZE))
            .child(sentence)
            .child(
                page::icon_action(
                    SharedString::from(format!("remove-trigger-{id}")),
                    "Remove trigger",
                    true,
                )
                .child(icon("close", page::ICON_SIZE))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(editor) = &mut this.editor {
                        editor
                            .draft
                            .triggers
                            .get_or_insert_with(Vec::new)
                            .retain(|t| t.id != id);
                        editor.dirty = true;
                    }
                    this.rebuild_trigger_fields(cx);
                    cx.notify();
                })),
            );
        let mut result = div().child(row);
        if trigger.kind != "time"
            && let Some(fields) = fields
        {
            result = result.child(
                page::meta()
                    .pl(px(24.))
                    .pb(px(8.))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child("Repositories")
                    .child(
                        page::field_frame()
                            .flex_grow(1.)
                            .flex_basis(px(120.))
                            .max_w(px(340.))
                            .child(fields.repos.clone()),
                    ),
            );
        }
        result.into_any_element()
    }
    pub(super) fn render_editor(&self, narrow: bool, cx: &mut Context<Self>) -> AnyElement {
        let editor = self.editor.as_ref().expect("editor");
        let draft = &editor.draft;
        let valid = !editor.name.read(cx).content.trim().is_empty()
            && !editor.prompt.read(cx).content.trim().is_empty()
            && !draft.cwd.trim().is_empty();
        let mut actions = div()
            .id("automation-editor-actions")
            .max_w_full()
            .flex_shrink_0()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(4.));
        if !editor.saved || editor.dirty {
            let label = if editor.saved { "Reset" } else { "Cancel" };
            actions = actions.child(
                page::secondary("automation-reset", label, false, false)
                    .child(label)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(id) = this.selected.clone() {
                            this.select(id, cx);
                        } else {
                            this.show_gallery(cx);
                        }
                    })),
            );
        }
        if editor.saved {
            let label = if self.running {
                "Starting…"
            } else {
                "Run now"
            };
            actions = actions.child(
                page::secondary("automation-run", label, false, self.running)
                    .child(icon("queue-play", page::ICON_SIZE))
                    .child(label)
                    .on_click(cx.listener(|this, _, _, cx| this.run(cx))),
            );
        }
        let disabled = !valid || self.saving || (editor.saved && !editor.dirty);
        let label = if self.saving {
            "Saving…"
        } else if editor.saved {
            "Save"
        } else {
            "Create"
        };
        actions = actions.child(
            page::primary("automation-save", label, disabled)
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !disabled {
                        this.save(cx);
                    }
                })),
        );
        let header = div()
            .flex_shrink_0()
            .border_b_1()
            .border_color(page::divider())
            .child(
                detail_frame(narrow)
                    .pb(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                page::title()
                                    .flex_grow(1.)
                                    .flex_basis(px(200.))
                                    .min_w_0()
                                    .child(editor.name.clone()),
                            )
                            .child(actions),
                    )
                    .child(
                        page::meta()
                            .min_w_0()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.))
                            .child(toggle("automation-enabled", draft.enabled).on_click(
                                cx.listener(|this, _, _, cx| {
                                    if let Some(e) = &mut this.editor {
                                        e.draft.enabled = !e.draft.enabled;
                                        e.dirty = true;
                                    }
                                    cx.notify();
                                }),
                            ))
                            .child(if draft.enabled { "Active" } else { "Inactive" })
                            .child(
                                page::action(
                                    "automation-project",
                                    "Choose automation project",
                                    true,
                                )
                                .relative()
                                .min_w_0()
                                .max_w_full()
                                .flex_shrink_1()
                                .child(self.project_anchor.probe())
                                .child(self.project_mark(&draft.cwd, page::ICON_SIZE))
                                .child(div().min_w_0().truncate().child(if draft.cwd.is_empty() {
                                    "Choose project".into()
                                } else {
                                    self.project_label(&draft.cwd)
                                }))
                                .child(
                                    icon("chevron-down", page::ICON_SIZE)
                                        .text_color(page::secondary_ink()),
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| this.choose_project(window, cx),
                                )),
                            )
                            .when(editor.saved, |d| {
                                d.child(
                                    page::icon_action(
                                        "automation-delete-menu",
                                        "Delete automation",
                                        true,
                                    )
                                    .child(icon("trash", page::ICON_SIZE))
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.confirm_delete = !this.confirm_delete;
                                            cx.notify();
                                        },
                                    )),
                                )
                            }),
                    )
                    .when(editor.saved, |d| {
                        d.child(div().flex().flex_wrap().gap(px(4.)).children(
                            [("Settings", false), ("Run history", true)].map(|(label, history)| {
                                let id = if history {
                                    "automation-history-tab"
                                } else {
                                    "automation-settings-tab"
                                };
                                page::tab(id, label, self.history == history)
                                    .child(label)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.history = history;
                                        this.choice = None;
                                        cx.notify();
                                    }))
                            }),
                        ))
                    }),
            );
        let panel = if self.history && editor.saved {
            self.render_history(narrow, cx)
        } else {
            self.render_settings(narrow, cx)
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(header)
            .when(self.confirm_delete, |d| {
                d.child(
                    page::label()
                        .id("automation-delete-confirmation")
                        .role(Role::Alert)
                        .mx(page::detail_padding(narrow))
                        .mt(px(12.))
                        .p(px(12.))
                        .rounded(px(radius::SURFACE))
                        .border_1()
                        .border_color(gpui::rgba(0xf8717140))
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(12.))
                        .child(
                            div()
                                .flex_1()
                                .child("Delete this automation and its run history?"),
                        )
                        .child(
                            page::secondary("cancel-delete-automation", "Cancel", false, false)
                                .child("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.confirm_delete = false;
                                    cx.notify();
                                })),
                        )
                        .child(
                            page::secondary("confirm-delete-automation", "Delete", true, false)
                                .child("Delete")
                                .on_click(cx.listener(|this, _, _, cx| this.delete(cx))),
                        ),
                )
            })
            .child(panel)
            .into_any_element()
    }
    fn render_settings(&self, narrow: bool, cx: &mut Context<Self>) -> AnyElement {
        let editor = self.editor.as_ref().expect("editor");
        let draft = &editor.draft;
        let triggers = draft.triggers.clone().unwrap_or_default();
        let triggers_section = div().child(section("Triggers")).child(
            div()
                .mt(px(8.))
                .rounded(px(radius::SURFACE))
                .border_1()
                .border_color(page::divider())
                .when(!triggers.is_empty(), |d| {
                    d.child(
                        div().px(px(8.)).py(px(4.)).children(
                            triggers
                                .iter()
                                .enumerate()
                                .map(|(i, t)| self.render_trigger(t, i, cx)),
                        ),
                    )
                    .child(div().h(px(1.)).bg(page::divider()))
                })
                .child(
                    page::action("automation-add-trigger", "Add trigger", triggers.len() < 20)
                        .relative()
                        .m(px(8.))
                        .justify_start()
                        .child(self.trigger_anchor.probe())
                        .child(icon("plus", page::ICON_SIZE))
                        .child("Add trigger")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_trigger_menu(window, cx);
                        })),
                ),
        );
        let kind = Kind::from_id(&draft.harness);
        let model = vorkspace_core::store::native_model(&draft.model, &draft.harness);
        let access = match draft.runtime_mode.as_str() {
            "supervised" => "Supervised",
            "auto-accept-edits" => "Auto-accept edits",
            "full-access" => "Full access",
            _ => "Auto",
        };
        let beside = cx
            .try_global::<vorkspace_ui::preferences::Preferences>()
            .is_some_and(|p| p.model_controls == "beside");
        let effort_label = (!beside)
            .then(|| {
                self.model_controls
                    .iter()
                    .find(|control| {
                        matches!(
                            control.id.as_str(),
                            "effort" | "reasoning_effort" | "reasoningEffort" | "reasoning"
                        )
                    })
                    .map(|control| control_label(control, draft))
            })
            .flatten();
        let mut controls = div()
            .px(px(8.))
            .pb(px(8.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(4.))
            .child(
                page::secondary("automation-model", "Choose automation model", false, false)
                    .relative()
                    .min_w_0()
                    .max_w_full()
                    .flex_shrink_1()
                    .child(self.model_anchor.probe())
                    .child(provider_icon(kind.id(), page::ICON_SIZE))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .child(cached_model_name(kind, model.as_deref())),
                    )
                    .when_some(effort_label, |d, effort| {
                        d.child(page::meta().child(effort))
                    })
                    .child(icon("chevron-down", page::ICON_SIZE).text_color(page::secondary_ink()))
                    .on_click(cx.listener(|this, _, window, cx| this.choose_model(window, cx))),
            );
        for (index, control) in self.model_controls.iter().enumerate().filter(|_| beside) {
            controls = controls.child(self.select_control(
                ("automation-model-setting", index),
                control_label(control, draft),
                Choice::ModelSetting(control.id.clone()),
                control.choices.clone(),
                cx,
            ));
        }
        controls = controls.child(self.select_control(
            "automation-access",
            access.into(),
            Choice::Access,
            options(&[
                ("supervised", "Supervised"),
                ("auto-accept-edits", "Auto-accept edits"),
                ("auto", "Auto"),
                ("full-access", "Full access"),
            ]),
            cx,
        ));
        let instructions = div()
            .child(section("Instructions"))
            .child(
                div()
                    .relative()
                    .mt(px(8.))
                    .rounded(px(radius::CONTAINER))
                    .border_1()
                    .border_color(page::divider())
                    .bg(content(0.03))
                    .child(editor.completion.read(cx).composer_probe())
                    .child(
                        div()
                            .px(px(12.))
                            .py(px(12.))
                            .child(editor.completion.clone()),
                    )
                    .child(controls),
            )
            .child(
                page::meta()
                    .mt(px(8.))
                    .child("Skills, @file references, and built-in commands work here."),
            );
        let mut folder_options = vec![(String::new(), "No folder".into())];
        folder_options.extend(self.folders.iter().map(|f| (f.id.clone(), f.name.clone())));
        if !draft.session_folder_id.is_empty()
            && !folder_options
                .iter()
                .any(|(id, _)| id == &draft.session_folder_id)
        {
            folder_options.push((
                draft.session_folder_id.clone(),
                draft.session_folder_id.clone(),
            ));
        }
        let folder_label = folder_options
            .iter()
            .find(|(id, _)| id == &draft.session_folder_id)
            .map(|(_, label)| label.clone())
            .unwrap_or_else(|| "No folder".into());
        let workspace = self.select_control(
            "automation-workspace",
            if draft.workspace_mode == "worktree" {
                "Fresh worktree"
            } else if draft.workspace_mode == "existing" {
                "Existing worktree"
            } else {
                "Current"
            }
            .into(),
            Choice::Workspace,
            options(&[("current", "Current"), ("worktree", "Fresh worktree")]),
            cx,
        );
        let conversation = self.select_control(
            "automation-conversation",
            if draft.reuse_session {
                "Continue last"
            } else {
                "Start fresh"
            }
            .into(),
            Choice::Conversation,
            if draft.workspace_mode == "worktree" {
                vec![("fresh".into(), "Start fresh".into())]
            } else {
                options(&[("fresh", "Start fresh"), ("reuse", "Continue last")])
            },
            cx,
        );
        let session = div().child(section("Session")).child(
            div()
                .mt(px(8.))
                .rounded(px(radius::SURFACE))
                .overflow_hidden()
                .border_1()
                .border_color(page::divider())
                .child(settings_row(
                    "Working copy",
                    "This repo, or a fresh worktree",
                    workspace,
                ))
                .child(settings_row(
                    "Conversation",
                    "New chat, or continue the last run",
                    conversation,
                ))
                .child(
                    settings_row(
                        "Session folder",
                        "Where runs appear in the sidebar",
                        self.select_control(
                            "automation-folder",
                            folder_label,
                            Choice::Folder,
                            folder_options,
                            cx,
                        ),
                    )
                    .border_b_0(),
                ),
        );
        let grace_options = options(&[
            ("0", "Do not catch up"),
            ("30", "30 minutes"),
            ("120", "2 hours"),
            ("720", "12 hours"),
            ("1440", "24 hours"),
        ]);
        let grace = grace_options
            .iter()
            .find(|(id, _)| *id == draft.missed_run_grace_minutes.to_string())
            .map(|(_, label)| label.clone())
            .unwrap_or_else(|| format!("{} minutes", draft.missed_run_grace_minutes));
        let advanced = div()
            .rounded(px(radius::SURFACE))
            .border_1()
            .border_color(page::divider())
            .child(
                crate::controls::button("automation-advanced", "Advanced", true)
                    .p(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .cursor_pointer()
                    .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.05)))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .child(page::label().child("Advanced"))
                            .child(page::meta().child("Catch-up window for missed runs")),
                    )
                    .child(icon(
                        if self.advanced {
                            "chevron-up"
                        } else {
                            "chevron-down"
                        },
                        page::ICON_SIZE,
                    ))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.advanced = !this.advanced;
                        cx.notify();
                    })),
            )
            .when(self.advanced, |d| {
                d.child(
                    settings_row(
                        "Missed-run grace",
                        "Catch up if a scheduled run was missed",
                        self.select_control(
                            "automation-grace",
                            grace,
                            Choice::Grace,
                            grace_options,
                            cx,
                        ),
                    )
                    .border_b_0()
                    .border_t_1(),
                )
            });
        div()
            .id("automation-settings-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.settings_scroll)
            .child(
                detail_frame(narrow)
                    .flex()
                    .flex_col()
                    .gap(px(24.))
                    .child(triggers_section)
                    .child(instructions)
                    .child(session)
                    .child(advanced),
            )
            .into_any_element()
    }
    fn render_history(&self, narrow: bool, cx: &mut Context<Self>) -> AnyElement {
        let triggered_width = px(140.);
        let status_width = px(96.);
        let duration_width = px(56.);
        let mut table = div()
            .mt(px(8.))
            .rounded(px(radius::SURFACE))
            .overflow_hidden()
            .border_1()
            .border_color(page::divider());
        if self.runs.is_empty() {
            table = table.child(
                page::empty(
                    icon("clock", 24.),
                    "No runs yet",
                    "Scheduled and manual runs appear here.",
                )
                .h_auto()
                .py(px(24.)),
            );
        } else {
            table = table.when(!narrow, |d| {
                d.child(
                    page::meta()
                        .h(px(page::TOOLBAR_HEIGHT))
                        .px(px(12.))
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .child(div().flex_1().min_w_0().truncate().child("Trigger"))
                        .child(div().w(triggered_width).child("Triggered"))
                        .child(div().w(status_width).child("Status"))
                        .child(div().w(duration_width).text_right().child("Duration")),
                )
            });
            for (index, run) in self.runs.iter().take(100).enumerate() {
                let session = run.session_id.clone();
                let trigger = if run.trigger == "manual" {
                    "Test run".into()
                } else if run.trigger == "event" {
                    automations::event_label(
                        run.event_kind.as_deref().unwrap_or(""),
                        run.event.as_deref().unwrap_or(""),
                    )
                } else {
                    self.editor
                        .as_ref()
                        .and_then(|e| e.draft.triggers.as_ref())
                        .and_then(|ts| ts.iter().find(|t| t.kind == "time"))
                        .map(|t| format!("Scheduled · {}", t.label()))
                        .unwrap_or_else(|| "Scheduled".into())
                };
                let (label, semantic) = match run.status.as_str() {
                    "succeeded" => ("Succeeded", Some(StatusColor::Success)),
                    "failed" => ("Failed", Some(StatusColor::Danger)),
                    "skipped" => ("Skipped", Some(StatusColor::Warning)),
                    "cancelled" => ("Cancelled", None),
                    "running" => ("Running", None),
                    _ => ("Pending", None),
                };
                let color = semantic.map_or(page::secondary_ink(), |role| status(role, 1.));
                let fill = semantic.map_or(content(0.08), |role| status(role, 0.12));
                let badge = page::meta()
                    .h(px(20.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .rounded(px(radius::CONTROL))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(color)
                    .bg(fill)
                    .when(run.status == "running", |d| {
                        d.child(icon("loader", 12.).text_color(color).with_animation(
                            SharedString::from(format!("automation-run-spinner-{index}")),
                            gpui::Animation::new(std::time::Duration::from_secs(1)).repeat(),
                            |icon, delta| {
                                icon.with_transformation(gpui::Transformation::rotate(
                                    gpui::percentage(delta),
                                ))
                            },
                        ))
                    })
                    .child(label);
                let metadata = page::meta()
                    .flex()
                    .items_center()
                    .when(narrow, |d| d.flex_wrap().gap(px(8.)))
                    .when(!narrow, |d| d.gap(px(12.)))
                    .child(div().when(!narrow, |d| d.w(triggered_width)).child(at(
                        if run.scheduled_for == 0 {
                            run.created_at
                        } else {
                            run.scheduled_for
                        },
                    )))
                    .child(
                        div()
                            .when(!narrow, |d| d.w(status_width))
                            .flex()
                            .items_center()
                            .child(badge),
                    )
                    .child(
                        div()
                            .when(!narrow, |d| d.w(duration_width).text_right())
                            .child(duration(run)),
                    );
                table = table.child(
                    crate::controls::button(
                        ("automation-run", index),
                        format!("{trigger}, {label}"),
                        session.is_some(),
                    )
                    .px(px(12.))
                    .py(px(8.))
                    .border_t_1()
                    .border_color(page::divider())
                    .flex()
                    .when(narrow, |d| d.flex_col().items_start().gap(px(4.)))
                    .when(!narrow, |d| d.items_center().gap(px(12.)))
                    .when(session.is_some(), |d| {
                        d.cursor_pointer()
                            .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.05)))
                    })
                    .child(
                        page::label()
                            .min_w_0()
                            .when(!narrow, |d| d.flex_1().truncate())
                            .child(trigger),
                    )
                    .child(metadata)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        if let Some(id) = &session {
                            cx.emit(Event::OpenSession(id.clone()));
                        }
                    })),
                );
                if let Some(error) = &run.error {
                    table = table.child(
                        page::meta()
                            .px(px(12.))
                            .pb(px(8.))
                            .text_color(status(StatusColor::Danger, 1.))
                            .child(error.clone()),
                    );
                }
            }
        }
        div()
            .id("automation-history-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.history_scroll)
            .child(
                detail_frame(narrow)
                    .child(section("Run history"))
                    .child(table),
            )
            .into_any_element()
    }
}
fn control_label(
    control: &vorkspace_core::catalog::ModelControl,
    draft: &AutomationUpsert,
) -> String {
    let value = control_value(control, draft);
    let label = control
        .choices
        .iter()
        .find(|(v, _)| v == value)
        .map(|(_, l)| l)
        .unwrap_or(value);
    match label.as_str() {
        "low" => "Low".into(),
        "medium" => "Medium".into(),
        "high" => "High".into(),
        "xhigh" => "Extra high".into(),
        "minimal" => "Minimal".into(),
        "none" => "None".into(),
        _ => label.clone(),
    }
}
fn control_value<'a>(
    control: &'a vorkspace_core::catalog::ModelControl,
    draft: &'a AutomationUpsert,
) -> &'a String {
    let aliases: &[&str] = match control.id.as_str() {
        "reasoningEffort" | "reasoning_effort" | "effort" | "reasoning" => {
            &["reasoningEffort", "reasoning_effort", "effort", "reasoning"]
        }
        "serviceTier" | "service_tier" => &["serviceTier", "service_tier"],
        _ => &[],
    };
    aliases
        .iter()
        .find_map(|key| draft.model_settings.get(*key))
        .or_else(|| draft.model_settings.get(&control.id))
        .unwrap_or(&control.current)
}
fn set_control_value(draft: &mut AutomationUpsert, key: String, value: String) {
    let (canonical, aliases): (&str, &[&str]) = match key.as_str() {
        "reasoningEffort" | "reasoning_effort" | "effort" | "reasoning" => (
            "reasoningEffort",
            &["reasoningEffort", "reasoning_effort", "effort", "reasoning"],
        ),
        "serviceTier" | "service_tier" => ("serviceTier", &["serviceTier", "service_tier"]),
        _ => (key.as_str(), &[]),
    };
    for alias in aliases {
        draft.model_settings.remove(*alias);
    }
    draft.model_settings.insert(canonical.to_string(), value);
}
fn gmt_offset() -> String {
    let minutes = Local::now().offset().local_minus_utc() / 60;
    let sign = if minutes >= 0 { '+' } else { '-' };
    let absolute = minutes.unsigned_abs();
    if absolute.is_multiple_of(60) {
        format!("GMT{sign}{}", absolute / 60)
    } else {
        format!("GMT{sign}{}:{:02}", absolute / 60, absolute % 60)
    }
}
fn next_run_preview(timestamp: i64) -> String {
    let Some(date) = Local
        .timestamp_millis_opt(timestamp)
        .single()
        .filter(|_| timestamp > 0)
    else {
        return "Next run —".into();
    };
    let mut zone = gmt_offset();
    {
        let seconds = (timestamp / 1000) as libc::time_t;
        let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
        // Use the system abbreviation (BST, PDT, etc.) for the scheduled date.
        if !unsafe { libc::localtime_r(&seconds, local.as_mut_ptr()) }.is_null() {
            let local = unsafe { local.assume_init() };
            let mut name = [0u8; 64];
            let length = unsafe {
                libc::strftime(name.as_mut_ptr().cast(), name.len(), c"%Z".as_ptr(), &local)
            };
            if length > 0 {
                zone = String::from_utf8_lossy(&name[..length]).into_owned();
            }
        }
    }
    format!("Next run {} {zone}", date.format("%a %-d %b, %H:%M"))
}
fn at(timestamp: i64) -> String {
    if timestamp <= 0 {
        return "—".into();
    }
    Local
        .timestamp_millis_opt(timestamp)
        .single()
        .map(|d| d.format("%-d %b, %H:%M").to_string())
        .unwrap_or_else(|| "—".into())
}
fn duration(run: &AutomationRun) -> String {
    let live = matches!(run.status.as_str(), "pending" | "running");
    let start = run
        .started_at
        .or_else(|| (run.status != "pending").then_some(run.created_at));
    let end = run
        .completed_at
        .or_else(|| (live && start.is_some()).then(|| vorkspace_core::time::now_ms() as i64));
    let (Some(start), Some(end)) = (start, end) else {
        return "—".into();
    };
    if end < start {
        return "—".into();
    }
    let minutes = end.saturating_sub(start) / 60_000;
    if minutes < 1 {
        "< 1m".into()
    } else if minutes < 60 {
        format!("{minutes}m")
    } else if minutes % 60 == 0 {
        format!("{}h", minutes / 60)
    } else {
        format!("{}h {}m", minutes / 60, minutes % 60)
    }
}
