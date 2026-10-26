use crate::settings::{SettingsEvent, SettingsView, widgets::secondary_button};
use gpui::{AnyElement, Context, prelude::*};
impl SettingsView {
    pub(super) fn vox_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let p = self.prefs.clone();
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
        let microphone_buttons = choices
            .into_iter()
            .map(|(id, label)| {
                let value = serde_json::json!({"id":id,"label":label});
                secondary_button(
                    gpui::SharedString::from(format!("vox-mic-{id}")),
                    label.clone(),
                    id == selected,
                    false,
                )
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.update(cx, |p| p.vox_microphone = value.clone())
                }))
                .into_any_element()
            })
            .collect();
        let mut rows = vec![
            self.row(Some("vox-enabled"),true,"Vox",Some("Start or end a voice call from the title bar. Uses your signed-in Codex account. Off by default.".into()),vec![self.toggle("vox-enabled","Enable Vox",p.vox_enabled,false,cx,|this,on,cx|this.update(cx,|p|p.vox_enabled=on))]),
            self.row(Some("vox-silent"),false,"Silent replies",Some("Show replies beside Vox while keeping its spoken audio muted.".into()),vec![self.toggle("vox-silent","Silent replies",p.vox_silent,false,cx,|this,on,cx|this.update(cx,|p|p.vox_silent=on))]),
            self.row(Some("vox-microphone"),false,"Microphone",Some("Applies to the next call. Devices appear after microphone access is granted. If a selected device disappears, Vox stops instead of switching inputs.".into()),microphone_buttons),
            self.row(Some("vox-conversation"),false,"Conversation",Some("Vox resumes the same conversation across calls. Start fresh clears its conversation identity and keeps its memory.".into()),vec![secondary_button("vox-reset","Start fresh",false,false).child("Start fresh").on_click(cx.listener(|_,_,window,cx|{
                let answer=window.prompt(gpui::PromptLevel::Warning,"Start a fresh Vox conversation?",Some("The current call will end. Saved memory is kept."),&["Cancel","Start fresh"],cx);
                cx.spawn(async move|this,cx|{if answer.await==Ok(1){let _=this.update(cx,|_,cx|cx.emit(SettingsEvent::VoxReset));}}).detach();
            })).into_any_element()]),
            self.row(Some("vox-memory"),false,"Memory",Some("Review and edit Vox's dated notes. Conflicting external changes are detected before saving.".into()),vec![secondary_button("vox-memory","Open memory",false,false).child("Open memory").on_click(cx.listener(|_,_,_,cx|cx.emit(SettingsEvent::VoxMemory))).into_any_element()]),
        ];
        if let Some(error) = &self.ctx.quick_error {
            rows.push(crate::settings::widgets::note(error.clone()));
        }
        vec![self.group(None, true, "Vox", None, None, rows)]
    }
}
