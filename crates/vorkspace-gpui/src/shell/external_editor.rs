use super::*;
use vorkspace_core::external_editor::{self, ExternalEditor};

impl Shell {
    /// Finds installed editors off the main thread; the first lookup
    /// may read the login shell's PATH.
    pub(super) fn scan_external_editors(&mut self, cx: &mut Context<Self>) {
        self.external_editor_scan = self.external_editor_scan.wrapping_add(1);
        let generation = self.external_editor_scan;
        let work = cx
            .background_executor()
            .spawn(async { external_editor::installed() });
        cx.spawn(async move |this, cx| {
            let editors = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.external_editor_scan == generation {
                    this.external_editors = Some(editors);
                    cx.notify();
                }
            });
        })
        .detach();
    }
    pub(super) fn external_editor_menu(&self, path: &str) -> MenuAction {
        let item = MenuAction::new("external-editor", "Open in editor").icon("app-window");
        let none =
            |label: &str| vec![MenuAction::new("external-editor:none", label).disabled(true)];
        if path.starts_with("remote://") {
            return item
                .disabled(true)
                .submenu(none("Remote folders stay on their host"));
        }
        match &self.external_editors {
            None => item.disabled(true).submenu(none("Looking for editors…")),
            Some(editors) if editors.is_empty() => item.submenu(none("No supported editors found")),
            Some(editors) => item.submenu(
                editors
                    .iter()
                    .map(|ExternalEditor { id, name }| {
                        MenuAction::new(format!("external-editor:{id}"), *name)
                    })
                    .collect(),
            ),
        }
    }
    pub(super) fn open_in_external_editor(
        &mut self,
        editor: &str,
        path: &str,
        cx: &mut Context<Self>,
    ) {
        let (editor, path) = (editor.to_owned(), path.to_owned());
        let work = cx
            .background_executor()
            .spawn(async move { external_editor::open(&editor, &path) });
        cx.spawn(async move |this, cx| {
            if let Err(error) = work.await {
                let _ = this.update(cx, |this, cx| {
                    this.error = Some(error);
                    cx.notify();
                });
            }
        })
        .detach();
    }
}
