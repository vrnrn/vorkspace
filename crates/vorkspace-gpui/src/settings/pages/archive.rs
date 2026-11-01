use crate::settings::{
    SettingsEvent, SettingsView,
    widgets::{note, secondary_button},
};
use gpui::{AnyElement, Context, Role, SharedString, div, prelude::*, px};
use vorkspace_ui::theme::*;
impl SettingsView {
    pub(super) fn archive_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = vec![self.row(
            Some("show-archived"),
            true,
            "Show archived in the sidebar",
            Some("Keep archived conversations listed alongside the active ones.".into()),
            vec![self.toggle(
                "show-archived",
                "Show archived in the sidebar",
                self.prefs.show_archived,
                false,
                cx,
                |this, on, cx| this.update(cx, |p| p.show_archived = on),
            )],
        )];
        if self.ctx.cwd.is_empty() {
            rows.push(note("Open a project to see its archived conversations."));
        } else if self.ctx.archived.is_empty() {
            rows.push(note("No archived conversations in this project."));
        }
        for session in &self.ctx.archived {
            let id = session.id;
            let date = chrono::DateTime::from_timestamp_millis(
                session.updated_at.min(i64::MAX as u64) as i64,
            )
            .filter(|_| session.updated_at > 0)
            .map(|d| d.format("%b %-d").to_string())
            .unwrap_or_default();
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .border_t_1()
                    .border_color(content(0.05))
                    .px_4()
                    .py(px(10.))
                    .child(provider_icon(&session.provider, 14.))
                    .child(
                        div()
                            .id(SharedString::from(format!("archived-session-{id}")))
                            .role(Role::Button)
                            .aria_label(session.title.clone())
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_size(px(13.))
                            .cursor_pointer()
                            .child(session.title.clone())
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(SettingsEvent::OpenSession(id))
                            })),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(content(0.35))
                            .child(date),
                    )
                    .child(
                        secondary_button(
                            SharedString::from(format!("unarchive-{id}")),
                            "Unarchive",
                            false,
                            false,
                        )
                        .child("Unarchive")
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(SettingsEvent::Archive(id, false))
                        })),
                    )
                    .child(
                        secondary_button(
                            SharedString::from(format!("delete-{id}")),
                            "Delete",
                            true,
                            false,
                        )
                        .child("Delete")
                        .on_click(
                            cx.listener(move |_, _, _, cx| cx.emit(SettingsEvent::Delete(id))),
                        ),
                    )
                    .into_any_element(),
            );
        }
        let title = if self.ctx.cwd.is_empty() {
            "Archived conversations".into()
        } else {
            format!("Archived in {}", self.ctx.project_label)
        };
        let mut projects = Vec::new();
        if self.ctx.archived_projects.is_empty() {
            projects.push(note("No archived projects."));
        }
        for path in &self.ctx.archived_projects {
            let path = path.clone();
            let restore = path.clone();
            projects.push(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .border_t_1()
                    .border_color(content(0.05))
                    .px_4()
                    .py(px(10.))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .text_size(px(13.))
                            .child(path.clone()),
                    )
                    .child(
                        secondary_button(
                            SharedString::from(format!("restore-project-{path}")),
                            "Restore project",
                            false,
                            false,
                        )
                        .debug_selector(move || format!("restore-project-{path}"))
                        .child("Restore")
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(SettingsEvent::RestoreProject(restore.clone()))
                        })),
                    )
                    .into_any_element(),
            );
        }
        vec![
            self.group(
                None,
                true,
                "Archived projects",
                Some("Archived projects keep their conversations and files.".into()),
                None,
                projects,
            ),
            self.group(None, false, SharedString::from(title), None, None, rows),
        ]
    }
}
