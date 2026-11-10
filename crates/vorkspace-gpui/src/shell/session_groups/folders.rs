//! Immutable folder projections and the project folder menu.
use super::*;
use vorkspace_core::session_groups::{Folder, Groups};

pub(in crate::shell) struct FolderIndex {
    groups: Groups,
    by_session: HashMap<i64, usize>,
    folded_names: Vec<String>,
}

impl FolderIndex {
    pub(in crate::shell) fn new(groups: Groups) -> Self {
        let mut by_session = HashMap::new();
        for (position, folder) in groups.folders.iter().enumerate() {
            for &id in &folder.session_ids {
                by_session.entry(id).or_insert(position);
            }
        }
        let folded_names = groups
            .folders
            .iter()
            .map(|folder| folder.name.to_lowercase())
            .collect();
        Self {
            groups,
            by_session,
            folded_names,
        }
    }

    pub(in crate::shell) fn saved(&self) -> &Groups {
        &self.groups
    }

    pub(in crate::shell) fn folder_of(&self, id: i64) -> Option<&Folder> {
        self.by_session
            .get(&id)
            .map(|&index| &self.groups.folders[index])
    }

    pub(in crate::shell) fn matches(&self, id: i64, query: &str) -> bool {
        self.by_session
            .get(&id)
            .is_some_and(|&index| self.folded_names[index].contains(query))
    }
}

impl Shell {
    pub(super) fn session_folder_menu(
        &self,
        ids: &[i64],
        project: &str,
        cx: &App,
    ) -> Option<MenuItem> {
        if self.one_folder_project(ids, project, cx) {
            let groups = self.groups.get(project);
            let mut choices = vec![MenuAction::new("folder-new", "New folder…")];
            if let Some(groups) = groups {
                choices.extend(groups.saved().folders.iter().map(|folder| {
                    MenuAction::new(format!("folder-move:{}", folder.id), folder.name.clone())
                        .disabled(ids.iter().all(|id| groups.folder_of(*id) == Some(folder)))
                }));
                if ids.iter().any(|id| groups.folder_of(*id).is_some()) {
                    choices.push(MenuAction::new("folder-unfile", "Remove from folder"));
                }
            }
            return Some(
                MenuAction::new("folders", "Move to folder")
                    .icon("folder")
                    .submenu(choices)
                    .into(),
            );
        }
        None
    }

    pub(super) fn one_folder_project(&self, ids: &[i64], project: &str, cx: &App) -> bool {
        ids.iter()
            .all(|id| self.session_metadata(*id, cx).summary.cwd == project)
    }

    pub(super) fn move_session_folder(
        &mut self,
        project: String,
        ids: Vec<i64>,
        destination: FolderDestination,
        cx: &mut Context<Self>,
    ) {
        let store = self.store.clone();
        let scope = project.clone();
        let work = cx.background_executor().spawn(async move {
            locked(&store)?.move_sessions_to_folder(&scope, &ids, destination)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => this.publish_groups(project, cx),
                Err(error) => {
                    this.error = Some(format!("Could not move conversations: {error}"));
                    cx.notify();
                }
            });
        })
        .detach();
    }
}
