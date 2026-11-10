//! Connect the project tools to the workspace sidebar and its checkout.
use super::*;

impl Shell {
    pub(super) fn sync_right_panel(&mut self, cx: &mut Context<Self>) {
        let tab = self.classic_tool_tab();
        let shown = self.workspace.sidebar_visible && self.settings.is_none() && tab.is_some();
        if let Some(panel) = &self.right_panel {
            panel.update(cx, |panel, cx| {
                if let Some(tab) = tab {
                    panel.restore_tab(tab, cx);
                }
                panel.set_shown(shown, cx);
            });
        }
    }

    pub(super) fn ensure_right_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.right_panel.is_some() {
            return;
        }
        let git = self.git.clone();
        let tab = self
            .classic_tool_tab()
            .unwrap_or(self.workspace.right_panel_tab);
        let view = self.workspace.changes_view;
        let panel = cx.new(|cx| RightPanel::new(git, tab, window, cx));
        let explorer = panel.read(cx).explorer.clone();
        self.observations.push(cx.subscribe(
            &explorer,
            |this, _, event: &crate::file_tools::explorer::FileActionError, cx| {
                this.notification_state
                    .file_error
                    .update(cx, |notice, cx| notice.set_message(event.0.clone(), cx));
                cx.notify();
            },
        ));
        self.observations.push(cx.subscribe_in(
            &explorer,
            window,
            |this, _, event: &crate::file_tools::explorer::OpenFile, window, cx| {
                this.open_review(
                    crate::git_tools::review::ReviewTab::File {
                        cwd: this.git.read(cx).project().into(),
                        path: event.0.clone(),
                    },
                    window,
                    cx,
                );
            },
        ));
        self.observations.push(cx.subscribe_in(
            &explorer,
            window,
            |this, _, event: &crate::file_tools::explorer::OpenSearchMatch, window, cx| {
                this.open_review_with_preview(
                    crate::git_tools::review::ReviewTab::File {
                        cwd: event.cwd.clone(),
                        path: event.matched.path.clone(),
                    },
                    Some(event.pin),
                    window,
                    cx,
                );
                if let Some(pane) = this.active_review(cx) {
                    pane.update(cx, |pane, cx| {
                        pane.navigate_file(
                            event.matched.line as usize,
                            event.matched.column as usize,
                            cx,
                        )
                    });
                }
            },
        ));
        let changes = panel.read(cx).changes.clone();
        changes.update(cx, |c, cx| c.set_view(view, cx));
        self.observations.push(cx.subscribe_in(
            &changes,
            window,
            |this, _, event: &crate::git_tools::changes::ChangesEvent, window, cx| match event {
                crate::git_tools::changes::ChangesEvent::View(view) => {
                    this.workspace.changes_view = *view;
                    this.persist(cx);
                }
                crate::git_tools::changes::ChangesEvent::OpenDiff { cwd, path, staged } => {
                    this.open_review(
                        crate::git_tools::review::ReviewTab::Changes {
                            cwd: cwd.clone(),
                            path: path.clone(),
                            staged: Some(*staged),
                        },
                        window,
                        cx,
                    );
                }
            },
        ));
        let history = panel.read(cx).history.clone();
        self.observations.push(cx.subscribe_in(
            &history,
            window,
            |this, _, event: &crate::git_tools::history::HistoryEvent, window, cx| {
                let crate::git_tools::history::HistoryEvent::OpenCommit {
                    cwd,
                    sha,
                    short_sha,
                    subject,
                } = event;
                this.open_review(
                    crate::git_tools::review::ReviewTab::Commit {
                        cwd: cwd.clone(),
                        sha: sha.clone(),
                        short_sha: short_sha.clone(),
                        subject: subject.clone(),
                    },
                    window,
                    cx,
                );
            },
        ));
        self.right_panel = Some(panel);
        self.sync_git(cx);
    }
}
