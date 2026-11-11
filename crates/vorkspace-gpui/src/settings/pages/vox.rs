use crate::settings::Preferences;

use crate::settings::{SettingsEvent, SettingsView, widgets::secondary_button};
use gpui::{AnyElement, Context, prelude::*};
impl SettingsView {
    pub(super) fn vox_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let p = self.prefs.clone();
        let shortcut = super::command_shortcut(&p, "App: Vox")
            .unwrap_or_else(|| "the shortcut in Keybindings".into());
        let microphone = self.vox_microphone_control(&p, cx);
        let mut rows = vec![
            self.row(
                Some("vox-enabled"),
                true,
                "Enable voice control",
                Some(format!("Open Vox at the notch with {shortcut}.").into()),
                vec![self.toggle(
                    "vox-enabled",
                    "Enable Vox",
                    p.vox_enabled,
                    false,
                    cx,
                    |this, on, cx| this.update(cx, |p| p.vox_enabled = on),
                )],
            ),
            self.row(
                Some("vox-silent"),
                false,
                "Silent replies",
                Some("Show replies as text without speaking them.".into()),
                vec![self.toggle(
                    "vox-silent",
                    "Silent replies",
                    p.vox_silent,
                    false,
                    cx,
                    |this, on, cx| this.update(cx, |p| p.vox_silent = on),
                )],
            ),
            self.row(
                Some("vox-microphone"),
                false,
                "Microphone",
                Some("Applies to the next call. If the device disconnects, the call stops.".into()),
                vec![microphone],
            ),
            self.row(
                Some("vox-conversation"),
                false,
                "Conversation",
                Some("Calls share one conversation. Start fresh keeps saved memory.".into()),
                vec![
                    secondary_button("vox-reset", "Start fresh", false, false)
                        .child("Start fresh")
                        .on_click(cx.listener(|_, _, window, cx| {
                            let answer = window.prompt(
                                gpui::PromptLevel::Warning,
                                "Start a fresh Vox conversation?",
                                Some("The current call will end. Saved memory is kept."),
                                &["Cancel", "Start fresh"],
                                cx,
                            );
                            cx.spawn(async move |this, cx| {
                                if answer.await == Ok(1) {
                                    let _ =
                                        this.update(cx, |_, cx| cx.emit(SettingsEvent::VoxReset));
                                }
                            })
                            .detach();
                        }))
                        .into_any_element(),
                ],
            ),
            self.row(
                Some("vox-memory"),
                false,
                "Memory",
                Some("Review and edit what Vox remembers.".into()),
                vec![
                    secondary_button("vox-memory", "Open memory", false, false)
                        .child("Open memory")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::VoxMemory)))
                        .into_any_element(),
                ],
            ),
        ];
        if let Some(error) = &self.ctx.vox_error {
            rows.push(crate::settings::widgets::note(error.clone()));
        }
        vec![self.card(None, rows).into_any_element()]
    }

    fn vox_microphone_control(&self, p: &Preferences, cx: &mut Context<Self>) -> AnyElement {
        let mut choices = vec![
            ("builtin".to_string(), "Built-in microphone".to_string()),
            ("system".into(), "System default".into()),
        ];
        choices.extend(self.ctx.vox_devices.clone());
        let selected = p.vox_microphone["id"].as_str().unwrap_or("builtin");
        if !choices.iter().any(|(id, _)| id == selected) {
            choices.push((
                selected.into(),
                p.vox_microphone["label"]
                    .as_str()
                    .unwrap_or("Selected microphone")
                    .into(),
            ));
        }
        self.select(
            "vox-microphone",
            "Microphone",
            selected,
            choices
                .iter()
                .map(|(id, label)| (id.clone(), label.clone().into(), None))
                .collect(),
            Some(208.),
            cx,
            move |this, id, cx| {
                if let Some((_, label)) = choices.iter().find(|(value, _)| *value == id) {
                    this.update(cx, |p| {
                        p.vox_microphone = serde_json::json!({"id": id, "label": label})
                    });
                }
            },
        )
    }
}
