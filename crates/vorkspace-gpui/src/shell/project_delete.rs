//! Conservative project removal: only exact local conversations are deleted.
use super::*;
use vorkspace_core::store::{Session, Store};

fn within(path: &str, root: &str) -> bool {
    let path = vorkspace_core::files::path_key(path);
    let root = vorkspace_core::files::path_key(root);
    path == root || std::path::Path::new(&path).starts_with(std::path::Path::new(&root))
}

fn scoped_sessions(
    path: &str,
    sessions: &[Session],
    worktrees: &HashMap<i64, Option<String>>,
) -> Result<Vec<i64>, String> {
    if path.starts_with("remote://") || !std::path::Path::new(path).is_absolute() {
        return Err("Only local projects with an absolute path can be deleted here.".into());
    }
    if std::path::Path::new(path).components().any(|c| {
        matches!(
            c,
            std::path::Component::ParentDir | std::path::Component::CurDir
        )
    }) {
        return Err("The project path is ambiguous; reopen it using its canonical path.".into());
    }
    let canonical = std::fs::canonicalize(path).ok();
    let key = vorkspace_core::files::path_key(path);
    let mut ids = Vec::new();
    for session in sessions {
        let cwd = vorkspace_core::files::path_key(&session.cwd);
        let worktree = worktrees.get(&session.id).and_then(|p| p.as_deref());
        if cwd != key
            && let (Some(root), Ok(candidate)) = (&canonical, std::fs::canonicalize(&session.cwd))
            && candidate.starts_with(root)
        {
            return Err("A conversation uses an alias or nested path in this project; remove it separately first.".into());
        }
        if (within(&session.cwd, path) && cwd != key)
            || worktree.is_some_and(|p| within(p, path) && cwd != key)
        {
            return Err("A conversation has a nested or ambiguous project/worktree. Move or remove it separately first.".into());
        }
        if cwd == key {
            if !worktrees.contains_key(&session.id) {
                return Err("Worktree ownership is unavailable for a conversation; try again after it reloads.".into());
            }
            if session.id < 0 || worktree.is_some_and(|p| vorkspace_core::files::path_key(p) != key)
            {
                return Err("A conversation belongs to a remote or separate worktree. Remove it separately first.".into());
            }
            ids.push(session.id);
        }
    }
    ids.sort_unstable();
    Ok(ids)
}

fn check_worktrees(path: &str) -> Result<(), String> {
    let directory = std::path::Path::new(path);
    if !directory.is_dir() {
        return Err(
            "The project folder is unavailable, so worktree ownership cannot be checked.".into(),
        );
    }
    let git = |args: &[&str]| -> Result<std::process::Output, String> {
        let mut command = std::process::Command::new("git");
        command
            .current_dir(directory)
            .args(args)
            .env("LC_ALL", "C")
            .env("GIT_TERMINAL_PROMPT", "0");
        for key in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_COMMON_DIR",
            "GIT_INDEX_FILE",
        ] {
            command.env_remove(key);
        }
        command
            .output()
            .map_err(|e| format!("Could not check project worktrees: {e}"))
    };
    let output = git(&["rev-parse", "--show-toplevel"])?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        if error.contains("not a git repository") {
            return Ok(());
        }
        return Err(format!(
            "Could not check project worktrees: {}",
            error.trim()
        ));
    }
    let root = String::from_utf8(output.stdout)
        .map_err(|e| format!("Could not read project root: {e}"))?;
    let project = std::fs::canonicalize(directory).map_err(|e| e.to_string())?;
    let root = std::fs::canonicalize(root.trim()).map_err(|e| e.to_string())?;
    if project != root {
        return Err(
            "This project is inside another Git working copy; its ownership is ambiguous.".into(),
        );
    }
    let worktrees = git(&["worktree", "list", "--porcelain", "-z"])?;
    if !worktrees.status.success() {
        return Err(format!(
            "Could not check project worktrees: {}",
            String::from_utf8_lossy(&worktrees.stderr).trim()
        ));
    }
    if worktrees
        .stdout
        .split(|b| *b == 0)
        .filter(|field| field.starts_with(b"worktree "))
        .count()
        != 1
    {
        return Err("This repository has linked worktrees. Remove their project associations separately first.".into());
    }
    Ok(())
}

fn check_bots(roster: &vorkspace_core::bot::Roster, path: &str, ids: &[i64]) -> Result<(), String> {
    if roster.bots.iter().any(|bot| {
        bot.projects.iter().any(|p| within(p, path))
            || bot.session_id.is_some_and(|id| ids.contains(&id))
            || bot
                .launches
                .iter()
                .any(|launch| ids.contains(&launch.session_id))
    }) || roster.hidden_sessions.iter().any(|id| ids.contains(id))
    {
        return Err("A Bot is associated with this project or its conversations; remove that association separately first.".into());
    }
    Ok(())
}

struct ProjectDeleteLease {
    store: Database,
    path: String,
}
impl Drop for ProjectDeleteLease {
    fn drop(&mut self) {
        if let Ok(mut store) = locked(&self.store) {
            store.end_project_deletion(&self.path);
        }
    }
}

struct DeleteResult {
    deleted: Vec<i64>,
    error: Option<String>,
}
fn delete_saved(
    store: &mut Store,
    path: &str,
    expected: &[i64],
    mut cleanup: impl FnMut(i64) -> Result<(), String>,
) -> DeleteResult {
    let mut deleted = Vec::new();
    let checked = store.sessions().and_then(|sessions| {
        store
            .session_worktrees()
            .and_then(|trees| scoped_sessions(path, &sessions, &trees))
    });
    match checked {
        Ok(ids) if ids == expected => {}
        Ok(_) => {
            return DeleteResult {
                deleted,
                error: Some(
                    "Conversations changed after confirmation; nothing was deleted. Try again."
                        .into(),
                ),
            };
        }
        Err(error) => {
            return DeleteResult {
                deleted,
                error: Some(error),
            };
        }
    }
    if let Err(error) = vorkspace_core::bot::Roster::load(store)
        .and_then(|roster| check_bots(&roster, path, expected))
    {
        return DeleteResult {
            deleted,
            error: Some(error),
        };
    }
    for id in expected {
        if let Err(error) = store.delete(*id) {
            return DeleteResult {
                deleted,
                error: Some(format!("Could not delete conversation {id}: {error}")),
            };
        }
        deleted.push(*id);
        if let Err(error) = cleanup(*id) {
            return DeleteResult {
                deleted,
                error: Some(format!(
                    "Conversation {id} was deleted, but checkpoint cleanup failed: {error}"
                )),
            };
        }
    }
    DeleteResult {
        deleted,
        error: None,
    }
}

impl Shell {
    pub(super) fn finish_project_creation(&mut self, key: &str) {
        if let Some(count) = self.pending_project_creations.get_mut(key) {
            *count -= 1;
            if *count == 0 {
                self.pending_project_creations.remove(key);
            }
        }
    }
    /// `before_release` skips what `release_project` will settle once deletion
    /// is confirmed: the selection, idle tabs and loaded views, worktree focus.
    fn project_delete_guard(
        &self,
        path: &str,
        ids: &[i64],
        before_release: bool,
        cx: &App,
    ) -> Result<(), String> {
        let key = vorkspace_core::files::path_key(path);
        if !self
            .workspace
            .projects
            .iter()
            .any(|p| vorkspace_core::files::path_key(p) == key)
            || self.deleting_projects.contains(&key)
        {
            return Err("The project is no longer available or is already being deleted.".into());
        }
        if !before_release && vorkspace_core::files::path_key(&self.workspace.project) == key {
            return Err("Switch to another project before deleting this project.".into());
        }
        if self
            .pending_project_creations
            .get(&key)
            .copied()
            .unwrap_or(0)
            > 0
            || self.quick.window.is_some()
        {
            return Err(
                "Wait for pending conversations and close Quick Composer before deleting.".into(),
            );
        }
        if let Some(bots) = &self.bots {
            check_bots(&bots.read(cx).roster, path, ids)?;
        }
        if ids.iter().any(|id| {
            self.deleting.contains(id)
                || self.busy_sessions.contains(id)
                || !before_release
                    && (self.workspace.tab_of(*id).is_some() || self.sessions.contains_key(id))
                || self
                    .workspace
                    .review_owners
                    .values()
                    .any(|owner| owner == id)
        }) {
            return Err("Close this project's conversations and file panes and stop active work. Cached conversations may require reopening the app before deletion.".into());
        }
        let Some(service) = &self.automation_state.service else {
            return Err(
                "Automations are not ready; project ownership cannot be checked yet.".into(),
            );
        };
        let mut service = service
            .try_lock()
            .map_err(|_| "Automations are busy; try again shortly.".to_string())?;
        if service.list()?.iter().any(|a| {
            within(&a.cwd, path) || (!a.worktree_cwd.is_empty() && within(&a.worktree_cwd, path))
        }) {
            return Err(
                "An automation is associated with this project; remove it separately first.".into(),
            );
        }
        drop(service);
        if self
            .terminals
            .iter()
            .any(|(p, terms)| within(p, path) && !terms.is_empty())
            || self
                .workspace
                .title_order
                .iter()
                .any(|tab| matches!(tab, TitleTab::Terminal { cwd, .. } if within(cwd, path)) || matches!(tab, TitleTab::Bot { project, worktree, .. } if within(project, path) || within(worktree, path)))
            || !before_release && self.worktree_focus.iter().any(|(p, _)| within(p, path))
        {
            return Err(
                "Close this project's terminals and worktree views before deleting it.".into(),
            );
        }
        Ok(())
    }
    /// Another project to select when removing `path`, which may be selected.
    fn other_project(&self, path: &str) -> Result<String, String> {
        let key = vorkspace_core::files::path_key(path);
        self.workspace
            .projects
            .iter()
            .find(|p| vorkspace_core::files::path_key(p) != key)
            .cloned()
            .ok_or_else(|| "Add another project before removing your only one.".into())
    }

    /// Selects another project if `path` is selected, so the project being
    /// removed can be the one on screen.
    fn leave_project(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if vorkspace_core::files::path_key(&self.workspace.project)
            == vorkspace_core::files::path_key(path)
        {
            let other = self.other_project(path)?;
            self.select_project(other, window, cx);
        }
        Ok(())
    }

    /// After confirming deletion: leave the project, close its idle tabs and
    /// drop its idle loaded views, which the deletion would discard anyway.
    /// Running conversations stay, and the guard then refuses.
    fn release_project(
        &mut self,
        path: &str,
        ids: &[i64],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = self.leave_project(path, window, cx) {
            self.error = Some(error);
            return;
        }
        let idle: Vec<_> = ids
            .iter()
            .copied()
            .filter(|id| !self.busy_sessions.contains(id))
            .collect();
        let open: Vec<_> = idle
            .iter()
            .copied()
            .filter(|id| self.workspace.tab_of(*id).is_some())
            .collect();
        self.close_tabs(&open, None, window, cx);
        for id in idle {
            if self.workspace.tab_of(id).is_none() {
                self.sessions.remove(&id);
            }
        }
        self.worktree_focus.retain(|p, _| !within(p, path));
    }

    /// Archiving hides a project; its idle tabs stay, hidden with it, and
    /// return when it is restored. Running work and terminals in it refuse.
    pub(super) fn archive_project(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = vorkspace_core::files::path_key(path);
        let busy = self.session_summaries.iter().any(|(id, summary)| {
            let worktree = self.session_worktrees.get(id).and_then(|p| p.as_deref());
            (within(&summary.cwd, path) || worktree.is_some_and(|p| within(p, path)))
                && self.session_metadata(*id, cx).busy
        });
        let terminals = self
            .terminals
            .iter()
            .any(|(p, terms)| within(p, path) && !terms.is_empty())
            || self
                .workspace
                .title_order
                .iter()
                .any(|tab| matches!(tab, TitleTab::Terminal { cwd, .. } if within(cwd, path)));
        if busy || terminals {
            self.error = Some(
                "Stop this project's running conversations and close its terminals before archiving it."
                    .into(),
            );
            cx.notify();
            return;
        }
        if let Err(error) = self.leave_project(path, window, cx) {
            self.error = Some(error);
            cx.notify();
            return;
        }
        let mut next = self.workspace.clone();
        next.projects
            .retain(|p| vorkspace_core::files::path_key(p) != key);
        next.project_archive.insert(key, true);
        self.commit_project_archive(next, cx);
    }

    pub(super) fn confirm_delete_project(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = self.workspace.migrate_project_rail() {
            self.error = Some(format!("Cannot remove project rail metadata: {error}"));
            cx.notify();
            return;
        }
        let sessions: Vec<_> = self.session_summaries.values().cloned().collect();
        let ids = match scoped_sessions(&path, &sessions, &self.session_worktrees).and_then(|ids| {
            self.project_delete_guard(&path, &ids, true, cx)?;
            self.other_project(&path)?;
            Ok(ids)
        }) {
            Ok(ids) => ids,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let detail = format!(
            "Permanently delete {} listed saved conversation(s) whose project is exactly this folder, along with their checkpoints and checkpoint Git refs, and remove the project from the rail. The folder, its working files, hidden conversations, and other project records remain. Projects with open work or ambiguous worktrees cannot be deleted here.",
            ids.len()
        );
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            &format!("Delete ‘{}’?", self.project_label(&path)),
            Some(&detail),
            &["Delete", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                // Only now, once confirmed, leave the project and let go of it.
                this.release_project(&path, &ids, window, cx);
                this.begin_project_delete(path, ids, cx)
            });
        })
        .detach();
    }
    fn begin_project_delete(&mut self, path: String, ids: Vec<i64>, cx: &mut Context<Self>) {
        let current: Vec<_> = self.session_summaries.values().cloned().collect();
        let guard = scoped_sessions(&path, &current, &self.session_worktrees).and_then(|now| {
            if now == ids {
                self.project_delete_guard(&path, &ids, false, cx)
            } else {
                Err("Conversations changed after confirmation; try again.".into())
            }
        });
        if let Err(error) = guard {
            self.error = Some(error);
            cx.notify();
            return;
        }
        let key = vorkspace_core::files::path_key(&path);
        if let Err(error) =
            locked(&self.store).and_then(|mut store| store.begin_project_deletion(&path))
        {
            self.error = Some(error);
            cx.notify();
            return;
        }
        self.deleting_projects.insert(key.clone());
        self.deleting.extend(ids.iter().copied());
        let previous = self.group_writes.take();
        let store = self.store.clone();
        let lease = ProjectDeleteLease {
            store: store.clone(),
            path: path.clone(),
        };
        self.group_writes = Some(cx.spawn(async move |this, cx| {
            let _lease = lease;
            if let Some(previous) = previous { previous.await; }
            let worker_path = path.clone(); let worker_ids = ids.clone(); let worker_store = store.clone();
            let work = cx.background_executor().spawn(async move {
                if let Err(error) = check_worktrees(&worker_path) { return DeleteResult { deleted: Vec::new(), error: Some(error) }; }
                if !worker_ids.is_empty() && vorkspace_core::checkpoint::store().is_none() {
                    return DeleteResult { deleted: Vec::new(), error: Some("Checkpoint storage is unavailable; no conversations were deleted.".into()) };
                }
                match locked(&worker_store) { Ok(mut store) => delete_saved(&mut store, &worker_path, &worker_ids, |id| {
                        if let Some(checkpoints) = vorkspace_core::checkpoint::store() { checkpoints.exclusive(|c| c.forget(&id.to_string())) } else { Ok(()) }
                    }), Err(error) => DeleteResult { deleted: Vec::new(), error: Some(error) } }
            });
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                for id in &ids { this.deleting.remove(id); }
                for id in &result.deleted { this.forget_session(*id, cx); }
                this.deleting_projects.remove(&key);
                let owner_check = if result.error.is_none() { this.project_delete_guard(&path, &ids, false, cx) } else { Ok(()) };
                let check = if result.error.is_none() { locked(&store).and_then(|store| store.sessions().and_then(|sessions| store.session_worktrees().and_then(|trees| scoped_sessions(&path, &sessions, &trees)))).and_then(|remaining| if remaining.is_empty() { Ok(()) } else { Err("New conversations appeared while deleting; the project was kept.".into()) }) } else { Ok(()) };
                let failure = result.error.or_else(|| owner_check.err()).or_else(|| check.err());
                if let Some(error) = failure {
                    this.error = Some(format!("Project kept. {} conversation(s) deleted before the failure. {error}", result.deleted.len()));
                } else {
                    this.workspace.projects.retain(|p| vorkspace_core::files::path_key(p) != key);
                    this.workspace.deleted_projects.insert(key.clone());
                    let rail = this.workspace.project_rail_mut();
                    rail.pinned.remove(&key);
                    rail.assignments.remove(&key);
                    if rail.awaiting_legacy_pins { rail.pin_overrides.insert(key.clone(), false); }
                    if rail.awaiting_legacy_assignments { rail.assignment_overrides.insert(key.clone(), None); }
                    this.workspace.project_focus.remove(&path);
                    this.persist(cx);
                }
                this.invalidate_chrome(cx); this.sync_settings(cx); cx.notify();
            });
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn git_worktree_check_accepts_plain_folder_and_refuses_nested_project() {
        let dir = tempfile::tempdir().unwrap();
        assert!(check_worktrees(dir.path().to_str().unwrap()).is_ok());
        let output = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(output.status.success());
        let child = dir.path().join("nested");
        std::fs::create_dir(&child).unwrap();
        assert!(
            check_worktrees(child.to_str().unwrap())
                .unwrap_err()
                .contains("inside another")
        );
        assert!(check_worktrees(dir.path().to_str().unwrap()).is_ok());
    }

    #[test]
    fn dropping_delete_lease_releases_admission() {
        let store = std::sync::Arc::new(std::sync::Mutex::new(
            Store::open(std::path::Path::new(":memory:")).unwrap(),
        ));
        locked(&store)
            .unwrap()
            .begin_project_deletion("/project")
            .unwrap();
        let lease = ProjectDeleteLease {
            store: store.clone(),
            path: "/project".into(),
        };
        assert!(locked(&store).unwrap().create("codex", "/project").is_err());
        drop(lease);
        assert!(locked(&store).unwrap().create("codex", "/project").is_ok());
    }

    #[test]
    fn delete_exact_sessions_preserves_folder_and_other_projects() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let file = project.join("important.txt");
        std::fs::write(&file, "keep").unwrap();
        let path = project.to_str().unwrap();
        let mut store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let a = store.create("codex", path).unwrap();
        let other = store.create("codex", "/other").unwrap();
        let result = delete_saved(&mut store, path, &[a.id], |_| Ok(()));
        assert!(result.error.is_none(), "{:?}", result.error);
        assert_eq!(result.deleted, vec![a.id]);
        assert_eq!(std::fs::read_to_string(file).unwrap(), "keep");
        assert_eq!(
            store
                .sessions()
                .unwrap()
                .iter()
                .map(|s| s.id)
                .collect::<Vec<_>>(),
            vec![other.id]
        );
    }
    #[test]
    fn changed_conversation_set_refuses_deletion() {
        let mut store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let a = store.create("codex", "/project").unwrap();
        let b = store.create("codex", "/project").unwrap();
        let result = delete_saved(&mut store, "/project", &[a.id], |_| Ok(()));
        assert!(result.error.unwrap().contains("changed"));
        assert!(result.deleted.is_empty());
        assert_eq!(store.sessions().unwrap().len(), 2);
        assert!(store.sessions().unwrap().iter().any(|s| s.id == b.id));
    }
    #[test]
    fn failure_after_one_delete_reports_exact_partial_result() {
        let mut store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let a = store.create("codex", "/project").unwrap();
        let b = store.create("codex", "/project").unwrap();
        let mut ids = vec![a.id, b.id];
        ids.sort_unstable();
        let result = delete_saved(&mut store, "/project", &ids, |_| Err("test failure".into()));
        assert_eq!(result.deleted, vec![ids[0]]);
        assert!(result.error.unwrap().contains("checkpoint cleanup failed"));
        assert_eq!(
            store
                .sessions()
                .unwrap()
                .iter()
                .map(|s| s.id)
                .collect::<Vec<_>>(),
            vec![ids[1]]
        );
    }
    #[test]
    fn bot_association_blocks_deletion() {
        let mut roster = vorkspace_core::bot::Roster::default();
        let mut bot = vorkspace_core::bot::Bot::default();
        bot.projects.push("/project".into());
        roster.bots.push(bot);
        assert!(check_bots(&roster, "/project", &[]).is_err());
        assert!(check_bots(&roster, "/other", &[]).is_ok());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_alias_to_project_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(&project, &alias).unwrap();
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let session = store.create("codex", alias.to_str().unwrap()).unwrap();
        assert!(scoped_sessions(project.to_str().unwrap(), &[session], &HashMap::new()).is_err());
    }
    #[gpui::test]
    fn delete_requires_confirmation_and_cancel_keeps_project(cx: &mut gpui::TestAppContext) {
        let (window, _, _) = crate::shell::tests::window(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.workspace.add_project("/tmp/delete-empty-fixture");
                shell.automation_state.service = Some(std::sync::Arc::new(std::sync::Mutex::new(
                    vorkspace_core::automations::Service::open(std::path::Path::new(":memory:"))
                        .unwrap(),
                )));
                shell.confirm_delete_project("/tmp/delete-empty-fixture".into(), w, cx);
            })
            .unwrap();
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert!(
                    shell
                        .workspace
                        .projects
                        .iter()
                        .any(|p| p == "/tmp/delete-empty-fixture")
                )
            })
            .unwrap();
    }
    #[gpui::test]
    fn confirmed_empty_project_removal_survives_workspace_restore(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().to_string_lossy().to_string();
        let (window, _, _) = crate::shell::tests::window(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.workspace.add_project(&project);
                shell.automation_state.service = Some(std::sync::Arc::new(std::sync::Mutex::new(
                    vorkspace_core::automations::Service::open(std::path::Path::new(":memory:"))
                        .unwrap(),
                )));
                shell.confirm_delete_project(project.clone(), w, cx);
            })
            .unwrap();
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Delete");
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert!(
                    !shell.workspace.projects.iter().any(|p| p == &project),
                    "{:?}",
                    shell.error
                );
                let saved = locked(&shell.store).unwrap().ui_state().unwrap().unwrap();
                let restored = Workspace::restore(Some(&saved), &[], "/work/vorkspace");
                assert!(!restored.projects.iter().any(|p| p == &project));
            })
            .unwrap();
    }
    #[gpui::test]
    fn selected_project_and_open_session_need_release_before_delete(cx: &mut gpui::TestAppContext) {
        let (window, a, _) = crate::shell::tests::window(cx);
        window
            .update(cx, |shell, _, cx| {
                let current = shell.workspace.project.clone();
                // Once released, a selected project can't be deleted out from
                // under the screen; before release that's left to release.
                assert!(
                    shell
                        .project_delete_guard(&current, &[], false, cx)
                        .unwrap_err()
                        .contains("Switch")
                );
                assert!(shell.project_delete_guard(&current, &[], true, cx).is_ok());
                shell.workspace.project = "/work/vorkspace".into();
                assert!(
                    shell
                        .project_delete_guard("/work/another-project", &[a], false, cx)
                        .is_err()
                );
            })
            .unwrap();
    }

    #[test]
    fn nested_and_worktree_sessions_are_refused() {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let nested = store.create("codex", "/project/sub").unwrap();
        assert!(scoped_sessions("/project", &[nested], &HashMap::new()).is_err());
        let main = store.create("codex", "/project").unwrap();
        let trees = HashMap::from([(main.id, Some("/separate-worktree".into()))]);
        assert!(scoped_sessions("/project", &[main], &trees).is_err());
        assert!(scoped_sessions("remote://host/project", &[], &HashMap::new()).is_err());
    }
}
