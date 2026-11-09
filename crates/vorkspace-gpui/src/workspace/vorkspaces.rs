//! A vorkspace is a named set of projects and Bots: the work a window is for.
//!
//! Membership is a view, not ownership. A folder may belong to several
//! vorkspaces and its sessions appear in each; removing it from one only
//! hides it there. The same holds for Bots, which keep running wherever
//! they are listed.
use super::Workspace;
use serde::{Deserialize, Serialize};
use vorkspace_core::files::path_key;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct VorkspaceDef {
    pub id: String,
    pub name: String,
    pub projects: Vec<String>,
    pub bots: Vec<String>,
    /// The vorkspace's color; unset, it is the hue its id hashes to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
    /// The chat background every session in this vorkspace shows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<vorkspace_core::project_appearance::Background>,
}

pub(crate) const FIRST_VORKSPACE_NAME: &str = "Main";

static NEXT_VORKSPACE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn new_id() -> String {
    let seq = NEXT_VORKSPACE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("vk-{nanos:x}-{seq:x}")
}

impl Workspace {
    pub(crate) fn is_project_available(&self, path: &str) -> bool {
        let key = path_key(path);
        !path.is_empty()
            && !self.deleted_projects.contains(&key)
            && self.project_archive.get(&key) != Some(&true)
    }

    /// The first launch after vorkspaces exist keeps everything visible by
    /// putting every known project into one vorkspace. Bots join through
    /// `assign_unowned_bots` once their roster loads.
    pub(crate) fn ensure_vorkspaces(&mut self) {
        if self.vorkspaces.is_empty() {
            let projects = self
                .projects
                .iter()
                .filter(|p| self.is_project_available(p))
                .cloned()
                .collect();
            self.vorkspaces.push(VorkspaceDef {
                id: new_id(),
                name: FIRST_VORKSPACE_NAME.into(),
                projects,
                bots: Vec::new(),
                ..Default::default()
            });
        }
        if !self
            .vorkspaces
            .iter()
            .any(|v| v.id == self.active_vorkspace)
        {
            self.active_vorkspace = self.vorkspaces[0].id.clone();
        }
    }

    pub(crate) fn active_vorkspace(&self) -> Option<&VorkspaceDef> {
        self.vorkspaces
            .iter()
            .find(|v| v.id == self.active_vorkspace)
    }

    /// Bots this window should not list: seen, and not in the active vorkspace.
    pub(crate) fn hidden_bots(&self) -> std::collections::HashSet<String> {
        let active = self.active_vorkspace();
        self.seen_bots
            .iter()
            .filter(|b| active.is_some_and(|v| !v.bots.contains(b)))
            .cloned()
            .collect()
    }

    pub(crate) fn vorkspace_mut(&mut self, id: &str) -> Option<&mut VorkspaceDef> {
        self.vorkspaces.iter_mut().find(|v| v.id == id)
    }

    /// The active vorkspace's projects that still exist, in its own order.
    pub(crate) fn vorkspace_projects(&self) -> Vec<String> {
        let Some(active) = self.active_vorkspace() else {
            return self
                .projects
                .iter()
                .filter(|p| self.is_project_available(p))
                .cloned()
                .collect();
        };
        active
            .projects
            .iter()
            .filter(|p| self.is_project_available(p))
            .cloned()
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn vorkspace_has_project(&self, path: &str) -> bool {
        let key = path_key(path);
        self.active_vorkspace()
            .is_none_or(|v| v.projects.iter().any(|p| path_key(p) == key))
    }

    #[cfg(test)]
    pub(crate) fn vorkspace_has_bot(&self, id: &str) -> bool {
        self.active_vorkspace()
            .is_none_or(|v| v.bots.iter().any(|b| b == id))
    }

    /// Adds a known folder to a vorkspace; also lists it as a project.
    pub(crate) fn add_project_to_vorkspace(&mut self, id: &str, path: &str) {
        if path.is_empty() {
            return;
        }
        self.restore_project(path);
        let key = path_key(path);
        if let Some(v) = self.vorkspace_mut(id)
            && !v.projects.iter().any(|p| path_key(p) == key)
        {
            v.projects.push(path.to_owned());
        }
    }

    pub(crate) fn remove_project_from_vorkspace(&mut self, id: &str, path: &str) {
        let key = path_key(path);
        if let Some(v) = self.vorkspace_mut(id) {
            v.projects.retain(|p| path_key(p) != key);
        }
        if self
            .vorkspace_project
            .get(id)
            .is_some_and(|p| path_key(p) == key)
        {
            self.vorkspace_project.remove(id);
        }
    }

    pub(crate) fn purge_project_from_vorkspaces(&mut self, path: &str) {
        let key = path_key(path);
        for v in &mut self.vorkspaces {
            v.projects.retain(|p| path_key(p) != key);
        }
        self.vorkspace_project.retain(|_, p| path_key(p) != key);
    }

    /// A Bot belongs to one vorkspace: adding it here takes it out of any other.
    pub(crate) fn move_bot_to_vorkspace(&mut self, id: &str, bot: &str) {
        for v in &mut self.vorkspaces {
            if v.id == id {
                if !v.bots.iter().any(|b| b == bot) {
                    v.bots.push(bot.to_owned());
                }
            } else {
                v.bots.retain(|b| b != bot);
            }
        }
        self.seen_bots.insert(bot.to_owned());
    }

    pub(crate) fn create_vorkspace(&mut self, name: &str) -> String {
        let id = new_id();
        self.vorkspaces.push(VorkspaceDef {
            id: id.clone(),
            name: name.to_owned(),
            ..Default::default()
        });
        id
    }

    /// The last vorkspace cannot be deleted; a window always has one.
    pub(crate) fn delete_vorkspace(&mut self, id: &str) -> bool {
        if self.vorkspaces.len() <= 1 {
            return false;
        }
        self.vorkspaces.retain(|v| v.id != id);
        self.vorkspace_project.remove(id);
        if self.active_vorkspace == id {
            self.active_vorkspace = self.vorkspaces[0].id.clone();
        }
        true
    }

    /// A Bot that belongs to no vorkspace, whether new or from before
    /// vorkspaces, joins the active one. Bots that no longer exist leave.
    /// Returns whether membership changed.
    pub(crate) fn assign_unowned_bots(&mut self, roster: &[String]) -> bool {
        let mut changed = false;
        for v in &mut self.vorkspaces {
            let before = v.bots.len();
            v.bots.retain(|b| roster.contains(b));
            changed |= v.bots.len() != before;
        }
        let before = self.seen_bots.len();
        self.seen_bots.retain(|b| roster.contains(b));
        changed |= self.seen_bots.len() != before;
        // Only a Bot never seen before is placed; one deliberately removed
        // from every vorkspace stays out until someone adds it back.
        let unowned: Vec<_> = roster
            .iter()
            .filter(|id| {
                !self.seen_bots.contains(*id)
                    && !self.vorkspaces.iter().any(|v| v.bots.contains(id))
            })
            .cloned()
            .collect();
        for id in roster {
            changed |= self.seen_bots.insert(id.clone());
        }
        if !unowned.is_empty()
            && let Some(active) = self
                .vorkspaces
                .iter_mut()
                .find(|v| v.id == self.active_vorkspace)
        {
            active.bots.extend(unowned);
            changed = true;
        }
        changed
    }

    /// The project to show on entering a vorkspace: the one it last showed,
    /// else its first.
    pub(crate) fn vorkspace_landing(&self, id: &str) -> String {
        let Some(v) = self.vorkspaces.iter().find(|v| v.id == id) else {
            return String::new();
        };
        self.vorkspace_project
            .get(id)
            .filter(|p| {
                self.is_project_available(p)
                    && v.projects.iter().any(|q| path_key(q) == path_key(p))
            })
            .cloned()
            .or_else(|| {
                v.projects
                    .iter()
                    .find(|p| self.is_project_available(p))
                    .cloned()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(projects: &[&str]) -> Workspace {
        let mut w = Workspace::default();
        for p in projects {
            w.add_project(p);
        }
        w.ensure_vorkspaces();
        w
    }

    #[test]
    fn existing_projects_migrate_into_one_vorkspace() {
        let w = workspace(&["/a", "/b"]);
        assert_eq!(w.vorkspaces.len(), 1);
        assert_eq!(w.active_vorkspace().unwrap().name, FIRST_VORKSPACE_NAME);
        assert_eq!(w.vorkspace_projects(), ["/a", "/b"]);
    }

    #[test]
    fn a_project_can_belong_to_several_vorkspaces() {
        let mut w = workspace(&["/a", "/b"]);
        let first = w.active_vorkspace.clone();
        let second = w.create_vorkspace("Side");
        w.add_project_to_vorkspace(&second, "/a");
        w.active_vorkspace = second.clone();
        assert_eq!(w.vorkspace_projects(), ["/a"]);
        assert!(!w.vorkspace_has_project("/b"));
        // Removing it here keeps it in the first vorkspace and in the app.
        w.remove_project_from_vorkspace(&second, "/a");
        assert!(w.vorkspace_projects().is_empty());
        w.active_vorkspace = first;
        assert_eq!(w.vorkspace_projects(), ["/a", "/b"]);
        assert!(w.projects.contains(&"/a".to_owned()));
    }

    #[test]
    fn bots_join_the_active_vorkspace_and_leave_when_deleted() {
        let mut w = workspace(&["/a"]);
        let first = w.active_vorkspace.clone();
        assert!(w.assign_unowned_bots(&["old".into()]));
        let second = w.create_vorkspace("Side");
        w.active_vorkspace = second.clone();
        assert!(w.assign_unowned_bots(&["old".into(), "new".into()]));
        assert!(w.vorkspace_has_bot("new"));
        assert!(!w.vorkspace_has_bot("old"));
        w.active_vorkspace = first.clone();
        assert!(w.vorkspace_has_bot("old"));
        assert!(w.assign_unowned_bots(&["new".into()]));
        assert!(!w.vorkspace_has_bot("old"));
        assert!(!w.assign_unowned_bots(&["new".into()]));
        // Removed from every vorkspace on purpose: it does not come back.
        w.vorkspaces.iter_mut().for_each(|v| v.bots.clear());
        assert!(!w.assign_unowned_bots(&["new".into()]));
        assert!(
            !w.vorkspaces
                .iter()
                .any(|v| v.bots.contains(&"new".to_owned()))
        );
    }

    #[test]
    fn a_bot_moves_rather_than_spreads_across_vorkspaces() {
        let mut w = workspace(&["/a"]);
        let first = w.active_vorkspace.clone();
        w.assign_unowned_bots(&["bot".into()]);
        let second = w.create_vorkspace("Side");
        w.move_bot_to_vorkspace(&second, "bot");
        let holders: Vec<_> = w
            .vorkspaces
            .iter()
            .filter(|v| v.bots.contains(&"bot".to_owned()))
            .map(|v| v.id.clone())
            .collect();
        assert_eq!(holders, [second]);
        assert!(!w.vorkspace_has_bot("bot"), "{first} no longer lists it");
    }

    #[test]
    fn the_last_vorkspace_stays_and_landing_follows_memory() {
        let mut w = workspace(&["/a", "/b"]);
        let only = w.active_vorkspace.clone();
        assert!(!w.delete_vorkspace(&only));
        assert_eq!(w.vorkspace_landing(&only), "/a");
        w.vorkspace_project.insert(only.clone(), "/b".into());
        assert_eq!(w.vorkspace_landing(&only), "/b");
        let other = w.create_vorkspace("Other");
        w.active_vorkspace = other.clone();
        assert!(w.delete_vorkspace(&other));
        assert_eq!(w.active_vorkspace, only);
    }

    #[test]
    fn saved_state_round_trips_vorkspaces() {
        let mut w = workspace(&["/a"]);
        let id = w.create_vorkspace("Side");
        w.add_project_to_vorkspace(&id, "/a");
        let restored: Workspace =
            serde_json::from_str(&serde_json::to_string(&w).unwrap()).unwrap();
        assert_eq!(restored.vorkspaces, w.vorkspaces);
        assert_eq!(restored.active_vorkspace, w.active_vorkspace);
    }
}
