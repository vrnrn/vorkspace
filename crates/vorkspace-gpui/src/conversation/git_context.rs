//! The session's checkout: worktrees, tool paths and Git state.
use super::*;

impl Conversation {
    pub(crate) fn worktree_deleted(
        &mut self,
        summary: Session,
        settings: Settings,
        cx: &mut Context<Self>,
    ) {
        self.summary = summary;
        self.settings = settings;
        self.context = None;
        self.status = "Worktree deleted".into();
        self.refresh_git(cx);
        cx.emit(SessionChanged);
        cx.notify();
    }
    pub(super) fn execution_cwd(&self) -> &str {
        if self.settings.worktree_removed {
            return &self.summary.cwd;
        }
        self.settings
            .worktree_cwd
            .as_deref()
            .unwrap_or(&self.summary.cwd)
    }
    pub(super) fn sync_tool_paths(&mut self, cx: &mut Context<Self>) {
        let cwd = self.execution_cwd().to_owned();
        self.transcript.update(cx, |view, cx| {
            if view.file_link_base.as_deref() != Some(cwd.as_str()) {
                view.file_link_base = Some(cwd);
                cx.notify();
            }
        });
    }
    /// The review card follows the checkout and rereads its recorded turn.
    pub(crate) fn sync_review(&mut self, cx: &mut Context<Self>) {
        self.sync_tool_paths(cx);
        if self.settings.bot.is_some() || self.is_remote() {
            return;
        }
        let cwd = self.execution_cwd().to_owned();
        self.review.update(cx, |review, cx| {
            if review.session().1 == cwd {
                review.refresh(cx);
            } else {
                review.set_cwd(cwd, cx);
            }
        });
    }
    pub(super) fn refresh_git(&mut self, cx: &mut Context<Self>) {
        self.sync_review(cx);
        let cwd = self.execution_cwd().to_owned();
        let work_cwd = cwd.clone();
        let work = cx
            .background_executor()
            .spawn(async move { vorkspace_model::git_cache::load(Path::new(&work_cwd)).ok() });
        cx.spawn(async move |this, cx| {
            let git = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.execution_cwd() == cwd {
                    this.branch = git.as_ref().and_then(|s| s.current.clone());
                    this.git = git;
                    cx.notify();
                }
            });
        })
        .detach();
    }
    pub(super) fn prepare_worktree(&mut self, cx: &mut Context<Self>) {
        self.preparing_worktree = true;
        self.status = "Creating worktree…".into();
        let cwd = self.summary.cwd.clone();
        let base = self
            .settings
            .worktree_base
            .clone()
            .or(self.branch.clone())
            .unwrap_or_else(|| "HEAD".into());
        let id = self.summary.id;
        let work = cx
            .background_executor()
            .spawn(async move { vorkspace_core::git::create_worktree(Path::new(&cwd), &base, id) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.preparing_worktree = false;
                match result {
                    Ok(path) => {
                        this.settings.worktree_cwd = Some(path.to_string_lossy().into_owned());
                        this.settings.new_worktree = false;
                        this.save_settings(cx);
                        this.refresh_git(cx);
                        if this.follow_ups.state.messages.is_empty() {
                            this.send(cx);
                        } else {
                            this.pump_follow_ups(cx);
                        }
                    }
                    Err(error) => {
                        this.status = error.clone();
                        if let Some(message) = this.follow_ups.state.messages.first() {
                            let id = message.id.clone();
                            this.fail_follow_up(&id, error, cx);
                        }
                        cx.notify();
                    }
                }
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn open_git(&mut self, mode: GitMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.preparing_worktree {
            return;
        }
        if self.git_picker.take().is_some() {
            self.git_events = None;
            window.focus(&self.input.focus_handle(cx), cx);
            cx.notify();
            return;
        }
        self.menu = None;
        let cwd = self.execution_cwd().into();
        let selected = if mode == GitMode::Base {
            self.settings.worktree_base.clone().or(self.branch.clone())
        } else {
            self.branch.clone()
        };
        let picker = cx.new(|cx| GitPicker::new(cwd, mode, selected, cx));
        self.git_events =
            Some(
                cx.subscribe_in(&picker, window, |this, _, event: &GitPicked, window, cx| {
                    match event {
                        GitPicked::Changed => {
                            if this.settings.worktree_removed {
                                this.settings.worktree_removed = false;
                                this.settings.worktree_cwd = None;
                                this.save_settings(cx);
                            }
                            this.refresh_git(cx);
                        }
                        GitPicked::Base(base) => {
                            this.settings.worktree_base = Some(base.clone());
                            this.save_settings(cx);
                        }
                        GitPicked::Worktree(path) => {
                            if !this.use_existing_worktree(path, cx) {
                                return;
                            }
                        }
                        GitPicked::Dismissed => {}
                    }
                    this.git_picker = None;
                    this.git_events = None;
                    window.focus(&this.input.focus_handle(cx), cx);
                    cx.notify();
                }),
            );
        window.focus(&picker.read(cx).focus(cx), cx);
        self.git_picker = Some(picker);
        cx.notify();
    }
    pub(super) fn use_existing_worktree(&mut self, path: &str, cx: &mut Context<Self>) -> bool {
        if self.summary.has_messages && !self.settings.worktree_removed {
            return false;
        }
        self.settings.worktree_removed = false;
        self.settings.worktree_cwd = (vorkspace_core::files::path_key(path)
            != vorkspace_core::files::path_key(&self.summary.cwd))
        .then(|| path.to_owned());
        self.settings.new_worktree = false;
        self.save_settings(cx);
        self.refresh_git(cx);
        true
    }
}
