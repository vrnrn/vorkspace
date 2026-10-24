use crate::settings::SettingsView;
#[cfg(any(target_os = "macos", test))]
use crate::settings::widgets::secondary_button;
#[cfg(any(target_os = "macos", test))]
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
        let description = "Keep up to 50 text and image copies in memory. History is never saved to disk. Turning this off clears it. Very large copies are skipped.";
        #[cfg(any(target_os = "macos", test))]
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
        #[cfg(any(target_os = "macos", test))]
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
        self.group(Some("clipboard"), false, "Clipboard", None, None, rows)
    }
}
