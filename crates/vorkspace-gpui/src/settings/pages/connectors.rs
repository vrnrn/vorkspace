use crate::{
    controls::secondary_button,
    input::Input,
    settings::{Section, SettingsEvent, SettingsView, widgets::note},
    workspace::Connector,
};
use gpui::{AnyElement, Context, Entity, SharedString, div, prelude::*, px};
use vorkspace_core::provider::Kind;
use vorkspace_meetings::{Meetings, Settings};
use vorkspace_ui::theme::content;

pub(in crate::settings) struct MeetingEditor {
    meetings: Meetings,
    provider: String,
    model: Entity<Input>,
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
                vec![self.feature_link(section, cx), toggle],
            ));
        }
        for section in [Section::Notes, Section::Meetings, Section::Todos] {
            let toggle = self.local_feature_toggle(section, cx);
            rows.push(self.row(
                None,
                false,
                section.label(),
                Some(section.description().into()),
                vec![self.feature_link(section, cx), toggle],
            ));
        }
        let mut groups = vec![self.group(
            None,
            true,
            "Built-in plugins",
            Some("Choose which features are available in your workspace.".into()),
            None,
            rows,
        )];
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
        .child("Settings")
        .on_click(cx.listener(move |_, _, _, cx| cx.emit(SettingsEvent::Section(section))))
        .into_any_element()
    }

    fn local_feature_toggle(&self, section: Section, cx: &mut Context<Self>) -> AnyElement {
        let enabled = match section {
            Section::Notes => self.prefs.notes,
            Section::Meetings => self.prefs.meetings,
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
                if section == Section::Meetings
                    && !enabled
                    && crate::clipboard::audio::owned(crate::meetings::OWNER, cx).is_some()
                {
                    this.meeting_toggle_error =
                        Some("Stop the current recording before turning off Meetings.".into());
                    cx.notify();
                    return;
                }
                this.meeting_toggle_error = None;
                this.update(cx, |p| match section {
                    Section::Notes => p.notes = enabled,
                    Section::Meetings => p.meetings = enabled,
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
            Section::Notes => {
                "Show Notes in workspace navigation. Use @note to mention a saved note."
            }
            Section::Todos => {
                "Show To-dos in workspace navigation. Saved tasks stay available when you turn it back on."
            }
            _ => "Show Meetings in workspace navigation. Saved transcripts and summaries are kept.",
        };
        let toggle = self.local_feature_toggle(section, cx);
        vec![self.row(
            Some(section.id()),
            true,
            section.label(),
            Some(description.into()),
            vec![toggle],
        )]
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
                input.set_text(settings.model, cx);
                input
            });
            let prompt = cx.new(|cx| {
                let mut input =
                    Input::multiline_field("Summary prompt", "Describe the notes you want", cx);
                input.set_text(settings.prompt, cx);
                input
            });
            self.meeting_editor = Some(MeetingEditor {
                meetings,
                provider: settings.provider,
                model,
                prompt,
                result: None,
            });
        }
        Ok(self.meeting_editor.as_ref().unwrap())
    }

    pub(super) fn meetings_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut page = self.local_feature_page(Section::Meetings, cx);
        page.extend(self.meeting_toggle_error.clone().map(note));
        let editor = match self.meeting_editor(cx) {
            Ok(editor) => editor,
            Err(error) => {
                page.push(note(error));
                return page;
            }
        };
        let (selected, model, prompt, result) = (
            editor.provider.clone(),
            editor.model.clone(),
            editor.prompt.clone(),
            editor.result.clone(),
        );
        let mut providers = vec![(String::new(), "Choose a provider".into(), None)];
        providers.extend(
            Kind::ALL
                .into_iter()
                .filter(|kind| {
                    kind.id() == selected
                        || !self.prefs.hidden_providers.iter().any(|id| id == kind.id())
                })
                .map(|kind| (kind.id().into(), kind.label().into(), None)),
        );
        let provider = self.select(
            "meeting-provider",
            "Summary provider",
            &selected,
            providers,
            Some(160.),
            cx,
            |this, value, cx| {
                if let Some(editor) = &mut this.meeting_editor {
                    editor.provider = value;
                    editor.result = None;
                }
                cx.notify();
            },
        );
        let models = Kind::parse(&selected)
            .and_then(|kind| self.discovered_models(kind, cx))
            .or_else(|| self.ctx.catalogs.get(&selected));
        let mut controls = vec![div().w(px(200.)).child(model.clone()).into_any_element()];
        if let Some(models) = models.filter(|models| !models.is_empty()) {
            let mut choices = vec![(String::new(), "Browse models".into(), None)];
            choices.extend(
                models
                    .iter()
                    .map(|(id, name)| (id.clone(), name.clone().into(), None)),
            );
            controls.insert(
                0,
                self.select(
                    "meeting-models",
                    "Browse models",
                    "",
                    choices,
                    Some(140.),
                    cx,
                    |this, id, cx| {
                        if !id.is_empty()
                            && let Some(editor) = &this.meeting_editor
                        {
                            editor.model.update(cx, |input, cx| input.set_text(id, cx));
                        }
                    },
                ),
            );
        }
        let save = secondary_button(
            "save-meeting-settings",
            "Save summary settings",
            true,
            false,
        )
        .child("Save")
        .on_click(cx.listener(|this, _, _, cx| {
            if let Some(editor) = &mut this.meeting_editor {
                let settings = Settings {
                    provider: editor.provider.clone(),
                    model: editor.model.read(cx).content.trim().into(),
                    prompt: editor.prompt.read(cx).content.to_string(),
                };
                editor.result = Some(editor.meetings.save_settings(&settings));
            }
            cx.notify();
        }));
        let rows = vec![
            self.row(
                Some("meeting-provider"),
                true,
                "Provider",
                Some("Uses the provider account selected for this project.".into()),
                vec![provider],
            ),
            self.row(
                None,
                false,
                "Model",
                Some("Choose a model or enter its ID.".into()),
                controls,
            ),
            div()
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .text_sm()
                .child("Summary prompt")
                .child(div().h(px(138.)).child(prompt))
                .child(div().flex().items_center().gap_3().child(save).when_some(
                    result,
                    |row, result| {
                        row.child(note(result.map_or_else(|error| error, |()| "Saved".into())))
                    },
                ))
                .into_any_element(),
        ];
        page.push(self.group(Some("meeting-prompt"), false, "AI summaries",
            Some("When you choose Summarize, the selected provider receives the transcript text. Audio stays on this Mac.".into()), None, rows));
        page
    }
}
