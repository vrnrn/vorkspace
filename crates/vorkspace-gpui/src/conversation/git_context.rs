//! The session's checkout: worktrees, tool paths and Git state.
use super::*;
use vorkspace_core::git::Snapshot;

#[derive(Default, PartialEq, Eq)]
pub(super) struct Head {
    pub(super) detached: bool,
    pub(super) linked: bool,
}

impl Head {
    pub(super) fn split(snapshot: Option<Snapshot>) -> (Option<String>, Option<Self>) {
        match snapshot {
            Some(Snapshot {
                current,
                detached,
                linked,
                ..
            }) => (current, Some(Self { detached, linked })),
            None => (None, None),
        }
    }
}

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
        cx.emit(SessionChanged::Checkout);
        cx.notify();
    }
    pub(crate) fn execution_cwd(&self) -> &str {
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
        if self.git_cwd != cwd {
            self.seed_git();
        }
        let revision = self.git_head_revision;
        let work_cwd = cwd.clone();
        let work = cx
            .background_executor()
            .spawn(async move { vorkspace_model::git_cache::load(Path::new(&work_cwd)).ok() });
        self.git_refresh = Some(cx.spawn(async move |this, cx| {
            let git = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.execution_cwd() != cwd {
                    return;
                }
                let (branch, head) = Head::split(git);
                if this.git_head_revision == revision {
                    this.set_git_head(branch, head, cx);
                } else if let (Some(observed), Some(loaded)) = (&mut this.git, head)
                    && observed.linked != loaded.linked
                {
                    observed.linked = loaded.linked;
                    cx.notify();
                }
            });
        }));
    }
    pub(super) fn seed_git(&mut self) {
        self.git_refresh = None;
        self.git_cwd = self.execution_cwd().to_owned();
        (self.branch, self.git) = Head::split(vorkspace_model::git_cache::get(&self.git_cwd));
    }

    pub(crate) fn observe_git_head(
        &mut self,
        event: &crate::git_status::GitHeadChanged,
        cx: &mut Context<Self>,
    ) {
        use vorkspace_core::{files::path_key, scm::LocalHead};
        if path_key(self.execution_cwd()) != path_key(&event.cwd)
            || (!self.loaded && self.summary.has_messages)
        {
            return;
        }
        if self.git_cwd != self.execution_cwd() {
            self.seed_git();
        }
        self.git_head_revision = self.git_head_revision.wrapping_add(1);
        let linked = self.git.as_ref().is_some_and(|head| head.linked)
            || (!self.settings.worktree_removed && self.settings.worktree_cwd.is_some());
        let (branch, detached) = match &event.head {
            LocalHead::Missing => {
                self.set_git_head(None, None, cx);
                return;
            }
            LocalHead::Attached(branch) => (Some(branch.clone()), false),
            LocalHead::Detached(abbreviation) => (abbreviation.clone(), true),
        };
        self.set_git_head(branch, Some(Head { detached, linked }), cx);
    }

    fn set_git_head(&mut self, branch: Option<String>, head: Option<Head>, cx: &mut Context<Self>) {
        if self.branch != branch || self.git != head {
            self.branch = branch;
            self.git = head;
            cx.notify();
        }
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
        if self.responding() || self.preparing_worktree {
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
                            cx.emit(SessionChanged::Checkout);
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
