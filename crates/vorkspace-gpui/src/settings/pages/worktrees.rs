use crate::settings::{
    SettingsView,
    management::Target,
    widgets::{page_note, secondary_button},
};
use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};
use vorkspace_ui::radius;
use vorkspace_ui::theme::*;
impl SettingsView {
    pub(super) fn worktrees_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let project = self.project_picker(cx);
        let ready = !self.discovery_loading
            && self.discovery.worktrees.error.is_none()
            && !self.discovery.worktrees.main.is_empty();
        let has_project = !self.selected_project.is_empty();
        // One toolbar holds every action; the explanations live in a single
        // footnote under the list instead of beside stray buttons.
        let toolbar = div()
            .relative()
            .child(self.reveal_probe("project-worktrees"))
            .flex()
            .items_center()
            .gap_1()
            .child(project)
            .child(
                secondary_button("create-settings-worktree", "Create worktree", false, !ready)
                    .child(icon("plus", 12.))
                    .child("Create worktree")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if ready {
                            this.open_management(Target::Worktree, window, cx);
                        }
                    })),
            )
            .when(has_project && ready, |d| {
                d.child(
                    secondary_button("worktree-setup", "Worktree setup command", false, false)
                        .child(icon("terminal", 12.))
                        .child("Setup command")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_management(Target::WorktreeSetup, window, cx)
                        })),
                )
            })
            .child(div().flex_1())
            .child(self.refresh_button("refresh-settings-worktrees", "Refresh", cx));
        let mut page = vec![toolbar.into_any_element()];
        self.load_worktree_sizes(cx);
        if self.selected_project.is_empty() {
            page.push(super::super::widgets::page_hint(
                "Add a project to manage its worktrees.",
            ));
        } else if self.discovery_loading {
            page.push(super::super::widgets::page_hint("Loading worktrees…"));
        } else if let Some(error) = &self.discovery.worktrees.error {
            page.push(
                div()
                    .id("settings-page-error")
                    .mt_4()
                    .role(gpui::Role::Alert)
                    .text_size(px(12.))
                    .text_color(vorkspace_ui::theme::danger())
                    .child(error.clone())
                    .into_any_element(),
            );
        } else if self.discovery.worktrees.trees.is_empty() {
            page.push(
                super::super::widgets::empty_card(
                    "folder-tree",
                    "No additional worktrees",
                    "Create a worktree to work on another branch in a separate folder.",
                )
                .mt_4()
                .into_any_element(),
            );
        } else {
            let trees = self.discovery.worktrees.trees.clone();
            page.push(
                div()
                    .mt_4()
                    .rounded(px(radius::SURFACE))
                    .border_1()
                    .border_color(content(0.1))
                    .overflow_hidden()
                    .children(trees.into_iter().enumerate().map(|(ix, tree)| {
                        let t = &tree.tree;
                        let path = std::path::PathBuf::from(&t.path);
                        let count = self
                            .ctx
                            .session_checkouts
                            .iter()
                            .filter(|p| {
                                vorkspace_core::files::is_equal_or_inside(p, &t.path)
                            })
                            .count();
                        let status = if t.missing {
                            "Missing folder"
                        } else if tree.assessment.dirty == Some(true) {
                            "Uncommitted changes"
                        } else if tree.assessment.dirty == Some(false) {
                            "Clean"
                        } else {
                            "Status unavailable"
                        };
                        let blocked = tree
                            .assessment
                            .discard_blocker(t.branch.is_none())
                            .is_some();
                        div()
                            .p_4()
                            .flex()
                            .items_start()
                            .gap_3()
                            .border_b_1()
                            .border_color(content(0.1))
                            .child(icon("folder-tree", 16.))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .child(
                                                path.file_name()
                                                    .unwrap_or_default()
                                                    .to_string_lossy()
                                                    .into_owned(),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .mt_1()
                                            .text_size(px(11.))
                                            .text_color(content(0.4))
                                            .child(t.path.clone()),
                                    )
                                    .child(
                                        div()
                                            .mt_2()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .text_size(px(11.))
                                            .text_color(content(0.55))
                                            .child(icon("git-branch", 12.))
                                            .child(
                                                t.branch
                                                    .as_ref()
                                                    .map(|b| format!("Current branch: {b}"))
                                                    .unwrap_or_else(|| {
                                                        format!(
                                                            "Detached at {}",
                                                            t.head
                                                                .chars()
                                                                .take(7)
                                                                .collect::<String>()
                                                        )
                                                    }),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .mt_2()
                                            .text_size(px(11.))
                                            .text_color(content(0.55))
                                            .child(format!(
                                                "{count} {} in this worktree · {status}{}{}",
                                                if count == 1 { "session" } else { "sessions" },
                                                if let Some(commits) = tree.assessment.unpushed.filter(|count| *count > 0) {
                                                    format!(" · {commits} unpublished commits")
                                                } else {
                                                    String::new()
                                                },
                                                if tree.assessment.locked == Some(true) { " · Locked" } else { "" }
                                            ))
                                            .when_some(self.worktree_sizes.get(&t.path), |row, size| {
                                                row.child(format!("{:.1} GB on disk", *size as f64 / 1e9))
                                            })
                                            .when(tree.assessment.safety_unknown(), |row| {
                                                row.child("Removal safety could not be verified")
                                            })
                                            .when(tree.assessment.removable() && count == 0, |row| {
                                                row.child("Safe to remove: nothing here would be lost")
                                            }),
                                    ),
                            )
                            .child(
                                secondary_button(
                                    SharedString::from(format!("reveal-tree-{ix}")),
                                    format!("Reveal {}", t.branch.as_deref().unwrap_or("worktree")),
                                    false,
                                    t.missing,
                                )
                                .child(icon("folder-open", 16.))
                                .on_click(move |_, _, cx| cx.reveal_path(&path)),
                            )
                            .child(
                                secondary_button(
                                    SharedString::from(format!("delete-tree-{ix}")),
                                    format!("Delete {}", t.branch.as_deref().unwrap_or("worktree")),
                                    false,
                                    blocked,
                                )
                                .child(icon("trash", 16.))
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        if !blocked {
                                            this.open_management(
                                                Target::DeleteTree(tree.clone()),
                                                window,
                                                cx,
                                            );
                                        }
                                    },
                                )),
                            )
                    }))
                    .into_any_element(),
            );
        }
        if ready {
            let setup = match self.prefs.worktree_setup.get(&self.selected_project) {
                Some(command) => format!("They run {command} after checkout."),
                None => "They only contain tracked files; a setup command can install \
                         dependencies or restore local files."
                    .into(),
            };
            page.push(
                div()
                    .mt_3()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(page_note(format!(
                        "New worktrees are created in {}. {setup}",
                        crate::panel_kit::home_relative(&self.discovery.worktrees.default_root)
                    )))
                    .child(page_note(
                        "Sessions can share a worktree. Deleting one discards uncommitted \
                         changes but keeps its branch, its commits and, by default, its sessions.",
                    ))
                    .into_any_element(),
            );
        }
        page
    }
}
