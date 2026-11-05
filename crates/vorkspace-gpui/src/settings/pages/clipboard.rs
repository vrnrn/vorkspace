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
        let description = "Keep up to 50 recent copies in memory. History clears when disabled, on logout, or when you quit from the clipboard menu. Large copies are skipped.";
        let description = if let Some(service) = crate::background_helper::get(cx)
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
                "History",
                Some("Clearing history keeps the current clipboard.".into()),
                vec![secondary_button("clipboard-clear-button", "Clear history", false, false)
                    .child("Clear history")
                    .on_click(|_, _, cx| crate::background_helper::clear(cx))
                    .into_any_element()],
            ));
            rows
        };
        self.group(Some("clipboard"), false, "Clipboard", None, None, rows)
    }
}
