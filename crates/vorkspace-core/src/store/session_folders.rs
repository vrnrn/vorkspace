//! Project folder membership and publication against the current saved groups.
use super::{shared, Store};
use crate::session_groups::{FolderDestination, Groups};
use rusqlite::{Transaction, TransactionBehavior};

impl Store {
    pub fn groups(&self, cwd: &str) -> Result<Groups, String> {
        self.groups_from(cwd, self.setting(&format!("groups:{cwd}"))?.as_deref())
    }

    fn groups_from(&self, cwd: &str, saved: Option<&str>) -> Result<Groups, String> {
        if let Some(saved) = saved {
            let decoded = serde_json::from_str(saved);
            return if self.shared.is_some() {
                decoded.map_err(|error| error.to_string())
            } else {
                Ok(decoded.unwrap_or_default())
            };
        }
        Ok(self
            .desktop_preferences()
            .and_then(|preferences| {
                preferences.groups(cwd, |id| self.resolve_existing_session_id(id))
            })
            .unwrap_or_default())
    }

    /// Merge with the latest project folders across independent stores.
    pub fn edit_groups(
        &self,
        cwd: &str,
        edit: impl FnOnce(&mut Groups) -> Result<(), String>,
    ) -> Result<(), String> {
        let key = format!("groups:{cwd}");
        self.update_settings(|settings| {
            let mut groups = self.groups_from(cwd, settings.get(&key).map(String::as_str))?;
            edit(&mut groups)?;
            Ok(((), vec![(key, shared::encode(&groups)?)]))
        })
    }

    /// Revalidate every selected conversation at the durable owner. The SQLite
    /// write fence precedes the settings lock, so local deletion or a checkout
    /// change cannot move a stale row between the scope check and publication.
    pub fn move_sessions_to_folder(
        &self,
        project: &str,
        sessions: &[i64],
        destination: FolderDestination,
    ) -> Result<(), String> {
        if sessions.is_empty() {
            return Ok(());
        }
        let tx = if self.shared.is_some() {
            self.shared_writable()?;
            Some(
                Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)
                    .map_err(|error| error.to_string())?,
            )
        } else {
            None
        };
        self.edit_groups(project, |groups| {
            for &id in sessions {
                let summary = if project.starts_with("remote://") {
                    self.remote_session(id)?.map(|remote| remote.summary)
                } else {
                    self.bot_session(id)?
                };
                if summary.is_none_or(|session| session.cwd != project) {
                    return Err(
                        "A selected conversation is outside this project or was removed.".into(),
                    );
                }
            }
            groups.move_sessions(sessions, destination)
        })?;
        if let Some(tx) = tx {
            tx.commit().map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_moves_are_atomic_for_selected_sessions_and_scoped_to_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("folders.db");
        let store = Store::open_vorkspace(&path).unwrap();
        let first = store.create("codex", "/alpha").unwrap().id;
        let second = store.create("pi", "/alpha").unwrap().id;
        let unrelated = store.create("codex", "/beta").unwrap().id;
        for (project, id) in [("/alpha", first), ("/beta", unrelated)] {
            store
                .edit_groups(project, |groups| {
                    let folder = groups.create_folder(&[id], "same-id".into()).unwrap();
                    groups.rename_folder(&folder, "Same name");
                    Ok(())
                })
                .unwrap();
        }
        let peer = Store::open_vorkspace(&path).unwrap();
        assert!(peer
            .move_sessions_to_folder(
                "/alpha",
                &[first, unrelated],
                FolderDestination::New("Mixed selection".into()),
            )
            .is_err());
        assert_eq!(
            store.groups("/alpha").unwrap().folder_of(first).unwrap().id,
            "same-id"
        );
        assert_eq!(
            store
                .groups("/beta")
                .unwrap()
                .folder_of(unrelated)
                .unwrap()
                .id,
            "same-id"
        );
        peer.move_sessions_to_folder(
            "/alpha",
            &[first, second],
            FolderDestination::New("New group".into()),
        )
        .unwrap();
        let groups = store.groups("/alpha").unwrap();
        assert_eq!(groups.folders.len(), 1);
        assert_eq!(groups.folders[0].name, "New group");
        assert_eq!(groups.folders[0].session_ids, [first, second]);
        peer.move_sessions_to_folder("/alpha", &[second], FolderDestination::Unfiled)
            .unwrap();
        assert_eq!(
            store.groups("/alpha").unwrap().folders[0].session_ids,
            [first]
        );
        assert_eq!(
            store.groups("/beta").unwrap().folders[0].session_ids,
            [unrelated]
        );
    }
}
