use crate::controls::secondary_button;
use crate::settings::SettingsView;
use gpui::prelude::*;
use gpui::{AnyElement, Context};

impl SettingsView {
    pub(super) fn clipboard_group(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let controls = vec![self.toggle(
            "clipboard-history",
            "Clipboard",
            self.prefs.clipboard_history,
            false,
            cx,
            |this, on, cx| this.update(cx, |p| p.clipboard_history = on),
        )];
        let description = "Keep up to 50 text and image copies in memory, even after you quit, until you log out or use Quit in the clipboard menu. History is never saved to disk. Turning this off clears it. Very large copies are skipped.";
        let description = if let Some(service) = crate::clipboard::get(cx)
            && let Some(error) = &service.read(cx).error
        {
            format!("{description}\n{error}")
        } else {
            description.to_owned()
        };
        let row = self.row(
            Some("clipboard-history"),
            true,
            "Keep recent copies",
            Some(description.into()),
            controls,
        );
        let rows = vec![row];
        let rows = {
            let mut rows = rows;
            rows.push(self.row(
                Some("clipboard-clear"),
                false,
                "Saved copies",
                Some("Clearing saved copies leaves the current system clipboard unchanged.".into()),
                vec![secondary_button("clipboard-clear-button", "Clear history", false, false)
                    .child("Clear history")
                    .on_click(|_, _, cx| crate::clipboard::clear(cx))
                    .into_any_element()],
            ));
            rows
        };
        let mut rows = rows;
        rows.push(self.row(Some("dictation-enabled"), true, "Local dictation", Some("Press ⌘, to start, then ⌘, to transcribe and insert into the selected text field. English, on your Mac. Works after quitting Vorkspace. Audio is never saved. Vox uses ⌘.".into()), vec![self.toggle("dictation-enabled", "Enable local dictation", self.prefs.dictation_enabled, false, cx, |this,on,cx|this.choose_dictation(on,cx))]));
        let (status, ready) = crate::clipboard::get(cx)
            .map(|s| {
                let s = s.read(cx);
                (s.dictation_status.clone(), s.dictation_ready)
            })
            .unwrap_or_default();
        let enabled = self.prefs.dictation_enabled;
        let setting_up = self.dictation_onboarding.is_some();
        rows.push(self.row(
            Some("dictation-model"),
            false,
            "English model · 731 MB",
            Some(if setting_up && self.dictation_onboarding.as_ref().is_some_and(|o|o.checking()) {
                "Checking model or connecting to background helper… Cancel to leave dictation off.".into()
            } else if status.is_empty() {
                "Enable dictation to download the model and set up permissions.".into()
            } else {
                status.into()
            }),
            vec![
                    secondary_button("dictation-download", "Download model", false, !enabled)
                        .child(if ready {
                            "Check model"
                        } else {
                            "Download model"
                        })
                        .on_click(
                            cx.listener(|this, _, _, cx| this.begin_dictation_setup(false, cx)),
                        )
                        .into_any_element(),
                    secondary_button(
                        "dictation-permissions",
                        "Set up permissions",
                        false,
                        !enabled,
                    )
                    .child("Set up permissions")
                    .on_click(|_, _, cx| crate::clipboard::dictation_permissions(cx))
                    .into_any_element(),
                    secondary_button("dictation-cancel", "Cancel dictation", false, !enabled && !setting_up)
                        .child("Cancel")
                        .on_click(cx.listener(|this, _, _, cx| {this.cancel_dictation_setup(cx);crate::clipboard::dictation_cancel(cx);}))
                        .into_any_element(),
                ],
        ));
        rows.push(self.row(None, false, "Permissions", Some("Allow Microphone and Accessibility for vorkspace Clipboard, the bundled background helper. If access was denied, enable it in System Settings → Privacy & Security. Keep the intended field focused until insertion completes.".into()), vec![]));
        self.group(
            Some("clipboard"),
            false,
            "Clipboard & Dictation",
            None,
            None,
            rows,
        )
    }
}
