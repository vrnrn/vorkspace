use crate::{
    controls::secondary_button,
    input::{self, Input},
    settings::{Section, SettingsEvent, SettingsView, widgets::note},
    workspace::Connector,
};
use gpui::{AnyElement, Context, Entity, SharedString, div, prelude::*, px};
use vorkspace_core::provider::{self, Kind};
use vorkspace_meetings::{Meetings, Settings};
use vorkspace_ui::theme::content;

pub(in crate::settings) struct MeetingEditor {
    meetings: Meetings,
    provider: String,
    model: String,
    custom_model: Entity<Input>,
    editing_custom_model: bool,
    effort: Option<String>,
    prompt: Entity<Input>,
    result: Option<Result<(), String>>,
}

impl SettingsView {
    pub(super) fn connectors_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = Vec::new();
        for (connector, section) in [
            (Connector::Github, Section::Github),
            (Connector::Gitlab, Section::Gitlab),
            (Connector::Jira, Section::Jira),
        ] {
            let id = connector.id();
            let toggle = self.toggle(
                match connector {
                    Connector::Github => "inbox-connector-github",
                    Connector::Gitlab => "inbox-connector-gitlab",
                    Connector::Jira => "inbox-connector-jira",
                },
                format!("Enable {}", section.label()),
                self.ctx.inbox_connectors.enabled(id),
                false,
                cx,
                move |_, enabled, cx| {
                    cx.emit(SettingsEvent::InboxConnectorEnabled(id.into(), enabled))
                },
            );
            rows.push(self.row(
                None,
                false,
                section.label(),
                Some(section.description().into()),
                if self.section_available(section) {
                    vec![self.feature_link(section, cx), toggle]
                } else {
                    vec![toggle]
                },
            ));
        }
        for section in [Section::Notes, Section::Todos] {
            let toggle = self.local_feature_toggle(section, cx);
            rows.push(self.row(
                None,
                false,
                section.label(),
                Some(section.description().into()),
                if self.section_available(section) {
                    vec![self.feature_link(section, cx), toggle]
                } else {
                    vec![toggle]
                },
            ));
        }
        let mut groups = vec![self.group(None, true, "Available features", None, None, rows)];
        groups.extend(self.meeting_toggle_error.clone().map(note));
        groups.push(self.project_notification_settings(cx));
        groups
    }

    fn feature_link(&self, section: Section, cx: &mut Context<Self>) -> AnyElement {
        secondary_button(
            SharedString::from(format!("open-{}-settings", section.id())),
            format!("Open {} settings", section.label()),
            false,
            false,
        )
        .debug_selector(move || format!("open-{}-settings", section.id()))
        .child("Settings")
        .on_click(cx.listener(move |_, _, _, cx| cx.emit(SettingsEvent::Section(section))))
        .into_any_element()
    }

    fn local_feature_toggle(&self, section: Section, cx: &mut Context<Self>) -> AnyElement {
        let enabled = match section {
            Section::Notes => self.prefs.notes,
            Section::Todos => self.prefs.todos,
            _ => unreachable!(),
        };
        self.toggle(
            section.id(),
            format!("Enable {}", section.label()),
            enabled,
            false,
            cx,
            move |this, enabled, cx| {
                if section == Section::Notes && !enabled && crate::meetings::capture::active(cx) {
                    this.meeting_toggle_error =
                        Some("Stop the current recording before turning off Notes.".into());
                    cx.notify();
                    return;
                }
                this.meeting_toggle_error = None;
                this.update(cx, |p| match section {
                    Section::Notes => p.notes = enabled,
                    Section::Todos => p.todos = enabled,
                    _ => unreachable!(),
                });
            },
        )
    }

    pub(super) fn local_feature_page(
        &self,
        section: Section,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let description = match section {
            Section::Notes => "Saved notes and recordings are kept when turned off.",
            Section::Todos => "Saved tasks are kept when turned off.",
            _ => unreachable!(),
        };
        let toggle = self.local_feature_toggle(section, cx);
        vec![
            self.card(
                None,
                vec![self.row(
                    Some(section.id()),
                    true,
                    match section {
                        Section::Notes => "Enable notes",
                        Section::Todos => "Enable to-dos",
                        _ => unreachable!(),
                    },
                    Some(description.into()),
                    vec![toggle],
                )],
            )
            .into_any_element(),
        ]
    }

    fn meeting_editor(&mut self, cx: &mut Context<Self>) -> Result<&MeetingEditor, String> {
        if self.meeting_editor.is_none() {
            let root = self
                .ctx
                .data_dir
                .as_ref()
                .ok_or("Meeting storage is unavailable.")?;
            let meetings = Meetings::open(root)?;
            let settings = meetings.settings()?;
            let model = cx.new(|cx| {
                let mut input = Input::field("Summary model", "Model ID", content(0.4), cx);
                input.set_max_chars(200);
                input.set_text(settings.model.clone(), cx);
                input
            });
            let prompt = cx.new(|cx| {
                let mut input =
                    Input::multiline_field("Summary prompt", "Describe the notes you want", cx);
                input.set_text(settings.prompt, cx);
                input.select_range(0..0, cx);
                input
            });
            self._subscriptions.push(cx.subscribe(
                &model,
                |this, input, _: &input::Changed, cx| {
                    if let Some(editor) = &mut this.meeting_editor {
                        let value = input.read(cx).content.trim().to_owned();
                        if editor.model != value {
                            editor.model = value;
                            editor.effort = None;
                            editor.result = None;
                            cx.notify();
                        }
                    }
                },
            ));
            self._subscriptions
                .push(cx.subscribe(&prompt, |this, _, _: &input::Changed, cx| {
                    if let Some(editor) = &mut this.meeting_editor {
                        editor.result = None;
                        cx.notify();
                    }
                }));
            self.meeting_editor = Some(MeetingEditor {
                meetings,
                provider: settings.provider,
                model: settings.model,
                custom_model: model,
                editing_custom_model: false,
                effort: settings.effort,
                prompt,
                result: None,
            });
        }
        Ok(self.meeting_editor.as_ref().unwrap())
    }

    pub(super) fn notes_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut page = self.local_feature_page(Section::Notes, cx);
        page.extend(self.meeting_toggle_error.clone().map(note));
        let editor = match self.meeting_editor(cx) {
            Ok(editor) => editor,
            Err(error) => {
                page.push(note(error));
                return page;
            }
        };
        let (selected, model, custom_model, editing_custom, effort, prompt, result) = (
            editor.provider.clone(),
            editor.model.clone(),
            editor.custom_model.clone(),
            editor.editing_custom_model,
            editor.effort.clone(),
            editor.prompt.clone(),
            editor.result.clone(),
        );
        let mut providers = vec![(String::new(), "Choose a provider".into(), None)];
        providers.extend(
            Kind::ALL
                .into_iter()
                .filter(|kind| {
                    provider::supports_text_only(*kind)
                        && (kind.id() == selected
                            || !self.prefs.hidden_providers.iter().any(|id| id == kind.id()))
                })
                .map(|kind| {
                    (
                        kind.id().into(),
                        kind.label().into(),
                        Some(vorkspace_ui::theme::provider_icon(kind.id(), 16.).into_any_element()),
                    )
                }),
        );
        let provider = self.select(
            "meeting-provider",
            "Summary provider",
            &selected,
            providers,
            Some(208.),
            cx,
            |this, value, cx| {
                if let Some(editor) = &mut this.meeting_editor
                    && editor.provider != value
                {
                    editor.provider = value;
                    editor.model.clear();
                    editor
                        .custom_model
                        .update(cx, |input, cx| input.set_text(String::new(), cx));
                    editor.editing_custom_model = false;
                    editor.effort = None;
                    editor.result = None;
                }
                cx.notify();
            },
        );
        let kind = Kind::parse(&selected).filter(|kind| provider::supports_text_only(*kind));
        // Keep model capabilities with the account-scoped catalog that supplied
        // the names. Never invent effort levels for an unknown/custom model.
        let models = kind.and_then(|kind| self.discovered_models(kind, cx));
        let info = models.and_then(|models| models.iter().find(|item| item.id == model));
        let mut choices = vec![(String::new(), "Choose a model".into(), None)];
        if let (Some(kind), Some(models)) = (kind, models) {
            choices.extend(models.iter().map(|model| {
                (
                    model.id.clone(),
                    crate::pickers::model_display_name(kind, &model.id, &model.name),
                    None,
                )
            }));
        } else if let Some(models) = self.ctx.catalogs.get(&selected) {
            choices.extend(
                models
                    .iter()
                    .map(|(id, name)| (id.clone(), name.clone().into(), None)),
            );
        }
        if !model.is_empty() && !choices.iter().any(|(id, _, _)| *id == model) {
            choices.push((model.clone(), model.clone().into(), None));
        }
        const CUSTOM_MODEL: &str = "\0custom";
        choices.push((CUSTOM_MODEL.into(), "Custom model…".into(), None));
        let model_picker = self.select(
            "meeting-models",
            "Summary model",
            if editing_custom { CUSTOM_MODEL } else { &model },
            choices,
            Some(208.),
            cx,
            |this, id, cx| {
                if let Some(editor) = &mut this.meeting_editor {
                    editor.editing_custom_model = id == CUSTOM_MODEL;
                    if !editor.editing_custom_model && editor.model != id {
                        editor.model = id.clone();
                        editor
                            .custom_model
                            .update(cx, |input, cx| input.set_text(id, cx));
                        editor.effort = None;
                        editor.result = None;
                    }
                }
                cx.notify();
            },
        );
        let mut model_controls = vec![model_picker];
        if editing_custom {
            model_controls.push(
                crate::controls::page::field_frame()
                    .debug_selector(|| "summary-custom-model".into())
                    .w(px(208.))
                    .child(custom_model)
                    .into_any_element(),
            );
        }
        let effort_supported = info.is_none_or(|model| !model.efforts.is_empty());
        let mut efforts = vec![(String::new(), "Default".into(), None)];
        if let Some(info) = info {
            efforts.extend(info.efforts.iter().map(|value| {
                (
                    value.clone(),
                    crate::conversation::effort_name(value).into(),
                    None,
                )
            }));
        }
        // Preserve a saved selection while discovery is unavailable; it remains
        // visible and can always be reset to the provider's default.
        if let Some(value) = &effort
            && !efforts.iter().any(|(id, _, _)| id == value)
        {
            efforts.push((
                value.clone(),
                crate::conversation::effort_name(value).into(),
                None,
            ));
        }
        let effort_picker = self.select(
            "meeting-effort",
            "Summary effort",
            effort.as_deref().unwrap_or(""),
            efforts,
            Some(208.),
            cx,
            |this, value, cx| {
                if let Some(editor) = &mut this.meeting_editor {
                    editor.effort = (!value.is_empty()).then_some(value);
                    editor.result = None;
                }
                cx.notify();
            },
        );
        let save = secondary_button(
            "save-meeting-settings",
            "Save summary settings",
            false,
            false,
        )
        .debug_selector(|| "save-meeting-settings".into())
        .child("Save")
        .on_click(cx.listener(|this, _, _, cx| {
            if let Some(editor) = &mut this.meeting_editor {
                let settings = Settings {
                    provider: editor.provider.clone(),
                    model: editor.model.clone(),
                    effort: editor.effort.clone(),
                    prompt: editor.prompt.read(cx).content.to_string(),
                };
                editor.result = Some(
                    if settings.provider.is_empty()
                        || Kind::parse(&settings.provider).is_some_and(provider::supports_text_only)
                    {
                        editor.meetings.save_settings(&settings)
                    } else {
                        Err("Choose a provider with a tool-free summary mode.".into())
                    },
                );
            }
            cx.notify();
        }));
        let mut rows = vec![
            self.row(
                Some("meeting-provider"),
                true,
                "Provider",
                Some("Uses the provider account selected for this project.".into()),
                vec![provider],
            ),
            self.row(None, false, "Model", None, model_controls),
        ];
        if !model.is_empty() && (effort_supported || effort.is_some()) {
            rows.push(self.row(
                Some("meeting-effort"),
                false,
                "Effort",
                None,
                vec![effort_picker],
            ));
        }
        rows.push(
            div()
                .px_4()
                .py(px(14.))
                .border_t_1()
                .border_color(content(0.05))
                .flex()
                .flex_col()
                .gap_3()
                .text_size(px(13.))
                .child("Summary prompt")
                .child(
                    crate::controls::page::field_frame()
                        .h(px(152.))
                        .child(prompt),
                )
                .child(div().flex().items_center().gap_3().child(save).when_some(
                    result,
                    |row, result| {
                        row.child(
                            div()
                                .text_size(px(12.))
                                .text_color(content(0.6))
                                .child(result.map_or_else(|error| error, |()| "Saved".into())),
                        )
                    },
                ))
                .into_any_element(),
        );
        page.push(self.group(
            Some("meeting-prompt"),
            false,
            "AI summaries",
            Some("Only transcript text is sent to the provider. Audio stays on this Mac.".into()),
            None,
            rows,
        ));
        page
    }
}
