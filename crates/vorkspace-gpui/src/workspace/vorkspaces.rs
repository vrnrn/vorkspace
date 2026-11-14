//! A vorkspace is a named set of projects and Bots: the work a window is for.
//!
//! Membership is a view, not ownership. A folder may belong to several
//! vorkspaces and its sessions appear in each; removing it from one only
//! hides it there. A Bot is listed in one vorkspace at a time, works on
//! every project of that vorkspace, and keeps running wherever it is listed.
use super::Workspace;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use unicode_segmentation::UnicodeSegmentation;
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
    #[serde(
        default,
        deserialize_with = "read_icon_letters",
        skip_serializing_if = "Option::is_none"
    )]
    icon_letters: Option<String>,
    /// The chat background every session in this vorkspace shows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<vorkspace_core::project_appearance::Background>,
}

impl VorkspaceDef {
    pub(crate) fn apply_window_edits(&mut self, before: &Self, edited: &Self) {
        if edited.name != before.name {
            self.name.clone_from(&edited.name);
        }
        if edited.color != before.color {
            self.color = edited.color;
        }
        if edited.icon_letters != before.icon_letters {
            self.icon_letters.clone_from(&edited.icon_letters);
        }
        if edited.background != before.background {
            self.background.clone_from(&edited.background);
        }
        if edited.projects != before.projects {
            let before: HashSet<_> = before.projects.iter().map(|path| path_key(path)).collect();
            let edited_keys: HashSet<_> =
                edited.projects.iter().map(|path| path_key(path)).collect();
            self.projects.retain(|path| {
                let key = path_key(path);
                !before.contains(&key) || edited_keys.contains(&key)
            });
            let mut current: HashSet<_> = self.projects.iter().map(|path| path_key(path)).collect();
            self.projects.extend(
                edited
                    .projects
                    .iter()
                    .filter(|path| {
                        let key = path_key(path);
                        !before.contains(&key) && current.insert(key)
                    })
                    .cloned(),
            );
        }
    }

    pub(crate) fn icon_letters(&self) -> &str {
        self.icon_letters.as_deref().unwrap_or_default()
    }

    pub(crate) fn set_icon_letters(&mut self, text: &str) -> bool {
        let next = normalize_icon_letters(text);
        if self.icon_letters == next {
            return false;
        }
        self.icon_letters = next;
        true
    }

    pub(crate) fn icon_text(&self) -> String {
        self.icon_letters.clone().unwrap_or_else(|| {
            self.name
                .chars()
                .find(|c| c.is_alphanumeric())
                .map_or_else(|| "V".into(), |c| c.to_uppercase().to_string())
        })
    }
}

fn normalize_icon_letters(text: &str) -> Option<String> {
    let letters: String = text
        .graphemes(true)
        // Preserve accents, but bound the bytes a combining sequence can store.
        .filter(|g| g.len() <= 32 && g.chars().next().is_some_and(char::is_alphabetic))
        .take(2)
        .collect();
    (!letters.is_empty()).then_some(letters)
}

fn read_icon_letters<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?
        .as_deref()
        .and_then(normalize_icon_letters))
}

#[test]
fn workspace_icon_letters_survive_rename_and_restore() {
    let mut def: VorkspaceDef = serde_json::from_value(serde_json::json!({
        "id": "test-vorkspace",
        "name": "Main"
    }))
    .unwrap();
    assert_eq!(def.icon_text(), "M");
    assert!(def.set_icon_letters("1a\u{301}WZ"));
    let saved = serde_json::to_string(&def).unwrap();
    let mut restored: VorkspaceDef = serde_json::from_str(&saved).unwrap();
    restored.name = "Side projects".into();
    assert_eq!(restored.icon_text(), "a\u{301}W");
    assert!(restored.set_icon_letters(""));
    assert_eq!(restored.icon_text(), "S");
    assert!(
        serde_json::to_value(&restored)
            .unwrap()
            .get("icon_letters")
            .is_none()
    );
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
    /// the shared roster once it loads.
    pub(crate) fn ensure_vorkspaces(&mut self) {
        if self.vorkspaces.is_empty() {
            let projects = self
                .projects
                .iter()
                .filter(|p| self.is_project_available(p))
                .cloned()
                .collect();
            Arc::make_mut(&mut self.vorkspaces).push(VorkspaceDef {
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
        let index = self.vorkspaces.iter().position(|v| v.id == id)?;
        Arc::make_mut(&mut self.vorkspaces).get_mut(index)
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

    pub(crate) fn vorkspace_has_project(&self, path: &str) -> bool {
        self.vorkspace_for_project(path) == Some(self.active_vorkspace.as_str())
    }

    pub(crate) fn vorkspace_has_bot(&self, id: &str) -> bool {
        self.active_vorkspace()
            .is_some_and(|v| v.bots.iter().any(|b| b == id))
    }

    pub(crate) fn bot_hidden_in_vorkspace(&self, id: &str) -> bool {
        // The roster can become available before first placement runs.
        self.seen_bots.contains(id) && !self.vorkspace_has_bot(id)
    }

    pub(crate) fn vorkspace_for_project(&self, path: &str) -> Option<&str> {
        if !self.is_project_available(path) {
            return None;
        }
        let key = path_key(path);
        self.active_vorkspace()
            .into_iter()
            .chain(self.vorkspaces.iter())
            .find(|v| v.projects.iter().any(|p| path_key(p) == key))
            .map(|v| v.id.as_str())
    }

    /// Restore the window within its saved vorkspace, even if an old snapshot
    /// paired that vorkspace with an unrelated project or session.
    pub(crate) fn reconcile_project_scope(&mut self) -> bool {
        self.reconcile_project_scope_where(|_| true)
    }

    pub(crate) fn reconcile_project_scope_where(&mut self, allowed: impl Fn(&str) -> bool) -> bool {
        if self.vorkspace_has_project(&self.project) && allowed(&self.project) {
            return false;
        }
        let landing = self.vorkspace_landing_where(&self.active_vorkspace, allowed);
        if self.project == landing {
            return false;
        }
        self.project = landing;
        self.active = None;
        true
    }

    /// Adds a folder to exactly the vorkspace `id`; also lists it as a
    /// project. Unlike `add_project`, a new folder does not also join the
    /// active vorkspace.
    pub(crate) fn add_project_to_vorkspace(&mut self, id: &str, path: &str) {
        if path.is_empty() {
            return;
        }
        self.deleted_projects.remove(&path_key(path));
        self.register_project(path);
        self.join_vorkspace(id, path);
    }

    /// Lists `path` in vorkspace `id` if it is not already there.
    pub(super) fn join_vorkspace(&mut self, id: &str, path: &str) {
        let key = path_key(path);
        if let Some(index) = self.vorkspaces.iter().position(|v| v.id == id)
            && !self.vorkspaces[index]
                .projects
                .iter()
                .any(|p| path_key(p) == key)
        {
            Arc::make_mut(&mut self.vorkspaces)[index]
                .projects
                .push(path.to_owned());
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
        for v in Arc::make_mut(&mut self.vorkspaces) {
            v.projects.retain(|p| path_key(p) != key);
        }
        self.vorkspace_project.retain(|_, p| path_key(p) != key);
    }

    /// A Bot belongs to one vorkspace: adding it here takes it out of any other.
    #[cfg(test)]
    pub(crate) fn move_bot_to_vorkspace(&mut self, id: &str, bot: &str) {
        move_bot(Arc::make_mut(&mut self.vorkspaces).as_mut_slice(), id, bot);
        Arc::make_mut(&mut self.seen_bots).insert(bot.to_owned());
    }

    pub(crate) fn create_vorkspace(&mut self, name: &str) -> String {
        let id = new_id();
        Arc::make_mut(&mut self.vorkspaces).push(VorkspaceDef {
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
        Arc::make_mut(&mut self.vorkspaces).retain(|v| v.id != id);
        self.vorkspace_project.remove(id);
        if self.active_vorkspace == id {
            self.active_vorkspace = self.vorkspaces[0].id.clone();
        }
        true
    }

    #[cfg(test)]
    pub(crate) fn assign_unowned_bots(&mut self, roster: &[String]) -> bool {
        reconcile_bots(
            Arc::make_mut(&mut self.vorkspaces).as_mut_slice(),
            Arc::make_mut(&mut self.seen_bots),
            &self.active_vorkspace,
            roster,
        )
    }

    /// The project to show on entering a vorkspace: the one it last showed,
    /// else its first.
    pub(crate) fn vorkspace_landing(&self, id: &str) -> String {
        self.vorkspace_landing_where(id, |_| true)
    }

    pub(crate) fn vorkspace_landing_where(
        &self,
        id: &str,
        allowed: impl Fn(&str) -> bool,
    ) -> String {
        let Some(v) = self.vorkspaces.iter().find(|v| v.id == id) else {
            return String::new();
        };
        self.vorkspace_project
            .get(id)
            .filter(|p| {
                self.is_project_available(p)
                    && allowed(p)
                    && v.projects.iter().any(|q| path_key(q) == path_key(p))
            })
            .cloned()
            .or_else(|| {
                v.projects
                    .iter()
                    .find(|p| self.is_project_available(p) && allowed(p))
                    .cloned()
            })
            .unwrap_or_default()
    }
}

pub(crate) fn move_bot(vorkspaces: &mut [VorkspaceDef], destination: &str, bot: &str) {
    for def in vorkspaces {
        if def.id == destination {
            if !def.bots.iter().any(|id| id == bot) {
                def.bots.push(bot.to_owned());
            }
        } else {
            def.bots.retain(|id| id != bot);
        }
    }
}

/// Place newly committed Bots and remove deleted ones from every vorkspace.
/// A deliberately unassigned Bot stays out until someone adds it again.
pub(crate) fn reconcile_bots(
    vorkspaces: &mut [VorkspaceDef],
    seen: &mut HashSet<String>,
    destination: &str,
    roster: &[String],
) -> bool {
    let current: HashSet<_> = roster.iter().collect();
    let mut changed = false;
    for def in vorkspaces.iter_mut() {
        let before = def.bots.len();
        def.bots.retain(|id| current.contains(id));
        changed |= def.bots.len() != before;
    }
    let before = seen.len();
    seen.retain(|id| current.contains(id));
    changed |= seen.len() != before;
    let unowned: Vec<_> = roster
        .iter()
        .filter(|id| !seen.contains(*id) && !vorkspaces.iter().any(|def| def.bots.contains(id)))
        .cloned()
        .collect();
    for id in roster {
        changed |= seen.insert(id.clone());
    }
    let landing = vorkspaces
        .iter()
        .position(|def| def.id == destination)
        .unwrap_or(0);
    if !unowned.is_empty()
        && let Some(def) = vorkspaces.get_mut(landing)
    {
        def.bots.extend(unowned);
        changed = true;
    }
    changed
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
    fn a_new_folder_added_to_another_vorkspace_stays_out_of_the_active_one() {
        let mut w = workspace(&["/a"]);
        let main = w.active_vorkspace.clone();
        let side = w.create_vorkspace("Side");
        w.add_project_to_vorkspace(&side, "/new");
        assert!(w.projects.contains(&"/new".to_owned()));
        assert_eq!(w.vorkspace_projects(), ["/a"], "Main does not gain it");
        w.active_vorkspace = side;
        assert_eq!(w.vorkspace_projects(), ["/new"]);
        // A folder the app has never seen, arriving without a target, still
        // joins the vorkspace in view.
        w.active_vorkspace = main;
        w.add_project("/other");
        assert_eq!(w.vorkspace_projects(), ["/a", "/other"]);
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
        Arc::make_mut(&mut w.vorkspaces)
            .iter_mut()
            .for_each(|v| v.bots.clear());
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
