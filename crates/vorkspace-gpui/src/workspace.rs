//! Viewport state: closing a tab never deletes or stops a session.
//! A title tab is keyed by one of its sessions; a tab holding more than one
//! pane keeps its split tree in `layouts` and its last focused pane in
//! `focused`. `active` is always the focused session.
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use vorkspace_model::pane_layout::{Edge, PaneNode, SplitDir, leaf};

static NEXT_TERMINAL_TAB_ID: AtomicU64 = AtomicU64::new(1);

/// Stable identity shared by sidebar navigation, tabs and restoration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Connector {
    Github,
    Gitlab,
    Jira,
}
impl Connector {
    pub const ALL: [Self; 3] = [Self::Github, Self::Gitlab, Self::Jira];
    pub fn id(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Gitlab => "gitlab",
            Self::Jira => "jira",
        }
    }
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|provider| provider.id() == id)
    }
    pub fn label(self) -> &'static str {
        vorkspace_core::inbox::provider_label(self.id())
    }
    pub fn icon(self) -> &'static str {
        match self {
            Self::Github => "inbox-github",
            Self::Gitlab => "inbox-gitlab",
            Self::Jira => "inbox-jira",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum AppView {
    /// Deserialize old workspaces, then normalize against saved connector preferences.
    Inbox,
    Connector(Connector),
    Notes,
    Automations,
    Todos,
}
impl AppView {
    pub fn label(self) -> &'static str {
        match self {
            Self::Inbox => "Connectors",
            Self::Connector(provider) => provider.label(),
            Self::Notes => "Notes",
            Self::Automations => "Automations",
            Self::Todos => "To-dos",
        }
    }
}

/// Customizable shortcuts, independent of title tabs and the sidebar's project tools.
/// Persist strings so unknown IDs can be discarded without losing the workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SidebarNavigation {
    Connector(Connector),
    Notes,
    Automations,
    Todos,
}
impl SidebarNavigation {
    pub const ALL: [Self; 6] = [
        Self::Connector(Connector::Github),
        Self::Connector(Connector::Gitlab),
        Self::Connector(Connector::Jira),
        Self::Notes,
        Self::Automations,
        Self::Todos,
    ];
    pub fn id(self) -> &'static str {
        match self {
            Self::Connector(provider) => provider.id(),
            Self::Notes => "notes",
            Self::Automations => "automations",
            Self::Todos => "todos",
        }
    }
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|item| item.id() == id)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Connector(provider) => provider.label(),
            Self::Notes => "Notes",
            Self::Automations => "Automations",
            Self::Todos => "To-dos",
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            Self::Connector(provider) => provider.icon(),
            Self::Notes => "note",
            Self::Automations => "zap",
            Self::Todos => "check-circle",
        }
    }
    pub fn normalize(order: &[String]) -> Vec<Self> {
        let mut items = Vec::with_capacity(Self::ALL.len());
        for item in order
            .iter()
            .filter_map(|id| Self::from_id(id))
            .chain(Self::ALL)
        {
            if !items.contains(&item) {
                items.push(item);
            }
        }
        items
    }
}

/// Navigation destinations, including sidebar pages and legacy saved entries.
/// Only sessions and terminals participate in the title strip.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum TitleTab {
    Session(i64),
    Search,
    App(AppView),
    /// A shell's identity is independent of its starting directory.
    /// Processes are never serialized; zero is reserved for cwd-only saves.
    #[serde(rename = "terminal")]
    Terminal {
        #[serde(default)]
        id: u64,
        cwd: String,
    },
    Bot {
        id: String,
        project: String,
        worktree: String,
    },
}

impl TitleTab {
    /// Only a project's sessions and terminals are tabs; Search, app views
    /// and Bots are sidebar pages. Older workspaces may still list them.
    pub fn in_strip(&self) -> bool {
        matches!(self, Self::Session(_) | Self::Terminal { .. })
    }
    pub fn session(&self) -> Option<i64> {
        match self {
            Self::Session(id) => Some(*id),
            Self::Bot { .. } | Self::Search | Self::App(_) | Self::Terminal { .. } => None,
        }
    }
}

/// The classic sidebar's three tools. History is a subview of Changes rather
/// than another top-level tab, but remains addressable for restored selection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SidebarTab {
    #[default]
    Sessions,
    Files,
    Changes,
    Graph,
    Todos,
}
impl SidebarTab {
    pub fn normalize(order: &[Self]) -> Vec<Self> {
        let mut tabs = Vec::new();
        for tab in order
            .iter()
            .chain(&[Self::Sessions, Self::Files, Self::Changes, Self::Todos])
        {
            if *tab != Self::Graph && !tabs.contains(tab) {
                tabs.push(*tab);
            }
        }
        tabs
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Sessions => "Sessions",
            Self::Files => "Explorer",
            Self::Changes => "Changes",
            Self::Graph => "History",
            Self::Todos => "To-dos",
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            Self::Sessions => "message-multiple",
            Self::Files => "folder",
            Self::Changes => "git-compare",
            Self::Graph => "git-branch",
            Self::Todos => "check-circle",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ProjectRailState {
    pub groups: Vec<ProjectRailGroup>,
    pub assignments: HashMap<String, String>,
    pub pinned: HashSet<String>,
    #[serde(default)]
    pub awaiting_legacy_groups: bool,
    #[serde(default)]
    pub awaiting_legacy_assignments: bool,
    #[serde(default)]
    pub awaiting_legacy_pins: bool,
    #[serde(default)]
    pub assignment_overrides: HashMap<String, Option<String>>,
    #[serde(default)]
    pub pin_overrides: HashMap<String, bool>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ProjectRailGroup {
    pub id: String,
    pub name: String,
    pub collapsed: bool,
}
impl ProjectRailState {
    fn from_metadata(metadata: &serde_json::Value) -> Result<Self, String> {
        fn read<T: serde::de::DeserializeOwned + Default>(
            metadata: &serde_json::Value,
            key: &str,
        ) -> Result<T, String> {
            match metadata.get(key) {
                None => Ok(T::default()),
                Some(value) => serde_json::from_str(
                    value
                        .as_str()
                        .ok_or_else(|| format!("Invalid legacy {key}"))?,
                )
                .map_err(|_| format!("Invalid legacy {key}")),
            }
        }
        let groups: Vec<ProjectRailGroup> = read(metadata, "vorkspace.projectGroups")?;
        let mut seen = HashSet::new();
        if groups
            .iter()
            .any(|g| g.id.is_empty() || g.name.is_empty() || !seen.insert(g.id.clone()))
        {
            return Err("Invalid legacy project groups".into());
        }
        let assignments: HashMap<String, String> =
            read(metadata, "vorkspace.projectGroupAssignments")?;
        let mut normalized = HashMap::new();
        for (path, group) in assignments {
            let key = vorkspace_core::files::path_key(&path);
            if normalized
                .insert(key, group.clone())
                .is_some_and(|previous| previous != group)
            {
                return Err("Conflicting legacy project assignments".into());
            }
        }
        let pinned: Vec<String> = read(metadata, "vorkspace.projectRailPinned")?;
        Ok(Self {
            groups,
            assignments: normalized,
            pinned: pinned
                .into_iter()
                .map(|p| vorkspace_core::files::path_key(&p))
                .collect(),
            awaiting_legacy_groups: metadata.get("vorkspace.projectGroups").is_none(),
            awaiting_legacy_assignments: metadata
                .get("vorkspace.projectGroupAssignments")
                .is_none(),
            awaiting_legacy_pins: metadata.get("vorkspace.projectRailPinned").is_none(),
            assignment_overrides: HashMap::new(),
            pin_overrides: HashMap::new(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Workspace {
    pub tabs: Vec<i64>,
    pub title_order: Vec<TitleTab>,
    pub scope_focus: HashMap<String, TitleTab>,
    /// Sidebar narrowing belongs to this run. Reopen on the full project,
    /// while saved conversations retain their own execution directories.
    #[serde(skip)]
    pub worktree_focus: HashMap<String, crate::git_tools::worktree_switcher::WorktreeFocus>,
    /// Directly opened chats join the workspace on screen without changing
    /// their execution directory. Like workspace selection, these pins are local
    /// to the current run and disappear when their tabs close.
    #[serde(skip)]
    pub tab_workspaces: HashMap<i64, String>,
    pub active: Option<i64>,
    pub projects: Vec<String>,
    /// Explicit decisions keyed by normalized path; false prevents stale legacy re-import.
    pub project_archive: HashMap<String, bool>,
    /// Explicit native removals suppress stale desktop project lists.
    pub deleted_projects: HashSet<String>,
    pub project: String,
    /// Last focused open session in each project, independent of tab order.
    pub project_focus: HashMap<String, i64>,
    pub recent_files: HashMap<String, Vec<String>>,
    pub collapsed: HashSet<String>,
    pub sidebar_visible: bool,
    /// Retained desktop preference; both current sidebars use right_panel_width.
    pub sidebar_width: f32,
    pub project_rail_open: bool,
    pub project_rail_width: f32,
    /// Explicit native group choices override the retained desktop defaults.
    pub project_group_collapsed: HashMap<String, bool>,
    /// None means the legacy source has not yet been available for migration.
    pub project_rail_state: Option<ProjectRailState>,
    pub sidebar_tabs: HashMap<String, SidebarTab>,
    pub sidebar_tab_order: Vec<SidebarTab>,
    /// Stable shortcut IDs; disabled or hidden rows keep their place.
    pub sidebar_navigation_order: Vec<String>,
    /// Sidebar visibility only; connector enablement, polling and tabs are independent.
    pub sidebar_hidden_connectors: HashSet<String>,
    /// Inert legacy density preference. Session cards use the current default.
    pub compact_session_cards: bool,
    pub sidebar_notes_visible: bool,
    pub sidebar_automations_visible: bool,
    pub sidebar_todos_visible: bool,
    pub sidebar_inbox_visible: bool,
    /// Show qualifying running and unseen finished sessions in the project rail.
    pub sidebar_activity_visible: bool,
    pub sidebar_subscriptions_visible: bool,
    /// The inset right panel: open state, tab, width and tab order.
    pub right_panel_open: bool,
    pub right_panel_tab: crate::right_panel::RightTab,
    /// Shared width for project tools and Bot details, independent of visibility.
    pub right_panel_width: f32,
    pub right_panel_tabs: Vec<crate::right_panel::RightTab>,
    pub changes_view: crate::git_tools::changes::ChangesView,
    /// Each tab's file/review pane beside its chat.
    pub reviews: std::collections::HashMap<i64, crate::git_tools::review::ReviewState>,
    /// Independent file panes use negative IDs in the layout tree. Their
    /// originating session supplies project/worktree scope and Add to Chat.
    pub review_owners: HashMap<i64, i64>,
    /// Split trees of tabs with more than one pane, by tab key.
    pub layouts: HashMap<i64, PaneNode>,
    /// The pane each split tab returns to when it is activated.
    pub focused: HashMap<i64, i64>,
    /// Each project's terminal dock (ProjectTerminalDock), by project path.
    pub docks: HashMap<String, TerminalDock>,
    /// The original desktop layout. Kept for older serialized workspaces;
    /// restored state always selects this layout.
    pub classic_layout: bool,
    pub preferences: crate::settings::Preferences,
    pub session_filters: vorkspace_core::session_filters::Filters,
    pub project_appearance: HashMap<String, vorkspace_core::project_appearance::ProjectAppearance>,
    /// Last desktop source values, used to reconcile subsequent desktop changes
    /// while retaining deliberate native project and preference edits.
    pub desktop_project_preferences: Option<serde_json::Value>,
    /// Unmodified desktop viewport metadata, including pane types and explicit
    /// tab-group assignments that do not yet have a native representation.
    pub desktop_workspace_snapshot: Option<serde_json::Value>,
    /// Exact source strings retained for preferences with no native equivalent.
    pub desktop_preferences_metadata: Option<serde_json::Value>,
}

/// Port of src/features/projects/model/projectTerminal.ts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DockSide {
    Top,
    #[default]
    Bottom,
    Left,
    Right,
}
impl DockSide {
    pub fn vertical(self) -> bool {
        matches!(self, DockSide::Top | DockSide::Bottom)
    }
    /// DOCK_SIZE_DEFAULT.
    pub fn default_size(self) -> f32 {
        if self.vertical() { 220. } else { 360. }
    }
    /// clampDockSize: at least a usable strip, at most 70% of the window.
    pub fn clamp(self, value: f32, viewport: (f32, f32)) -> f32 {
        if !value.is_finite() {
            return self.default_size();
        }
        let (min, max) = self.limits(viewport);
        value.round().clamp(min, max)
    }
    pub fn limits(self, viewport: (f32, f32)) -> (f32, f32) {
        let (min, span) = if self.vertical() {
            (88., viewport.1)
        } else {
            (180., viewport.0)
        };
        (min, (span * 0.7).floor().max(min))
    }
}
/// A project's dock: its place, size and the terminals it holds. Shells are
/// not restored; the dock reopens the same tabs in their directories.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct TerminalDock {
    pub side: DockSide,
    pub size: f32,
    pub open: bool,
    /// Each terminal's working directory, in tab order.
    pub terminals: Vec<String>,
    pub active: usize,
}
impl Default for TerminalDock {
    fn default() -> Self {
        Self {
            side: DockSide::Bottom,
            size: DockSide::Bottom.default_size(),
            open: true,
            terminals: Vec::new(),
            active: 0,
        }
    }
}
impl TerminalDock {
    /// addTerminalToDock: the new terminal is active and the dock opens.
    pub fn add(&mut self, cwd: String) -> usize {
        self.terminals.push(cwd);
        self.active = self.terminals.len() - 1;
        self.open = true;
        self.active
    }
    /// closeTerminalInDock: the next tab takes over a closed active one.
    /// False once the last terminal is gone.
    pub fn close(&mut self, ix: usize) -> bool {
        if ix >= self.terminals.len() {
            return true;
        }
        self.terminals.remove(ix);
        if self.terminals.is_empty() {
            self.active = 0;
            return false;
        }
        if self.active > ix || self.active >= self.terminals.len() {
            self.active = self.active.saturating_sub(1).min(self.terminals.len() - 1);
        }
        true
    }
    /// onReorderTerminals: the active terminal stays active wherever it lands.
    pub fn reorder(&mut self, from: usize, to: usize) {
        let len = self.terminals.len();
        if from >= len || to >= len || from == to {
            return;
        }
        let active = self.active;
        let cwd = self.terminals.remove(from);
        self.terminals.insert(to, cwd);
        self.active = if active == from {
            to
        } else if from < active && active <= to {
            active - 1
        } else if to <= active && active < from {
            active + 1
        } else {
            active
        };
    }
    /// onCloseOtherProjectTerminals: `keep` is left, and active.
    pub fn keep_only(&mut self, keep: usize) {
        if keep < self.terminals.len() {
            self.terminals = vec![self.terminals.swap_remove(keep)];
            self.active = 0;
        }
    }
    pub fn set_side(&mut self, side: DockSide) {
        if side != self.side {
            // A dock moved across axes starts from that axis's default.
            if side.vertical() != self.side.vertical() {
                self.size = side.default_size();
            }
            self.side = side;
        }
    }
}
impl Default for Workspace {
    fn default() -> Self {
        Self {
            tabs: Vec::new(),
            title_order: Vec::new(),
            scope_focus: HashMap::new(),
            worktree_focus: HashMap::new(),
            tab_workspaces: HashMap::new(),
            active: None,
            projects: Vec::new(),
            project_archive: HashMap::new(),
            deleted_projects: HashSet::new(),
            project: String::new(),
            project_focus: HashMap::new(),
            recent_files: HashMap::new(),
            collapsed: HashSet::new(),
            sidebar_visible: true,
            sidebar_width: 260.,
            project_rail_open: true,
            project_rail_width: 200.,
            project_group_collapsed: HashMap::new(),
            project_rail_state: None,
            sidebar_tabs: HashMap::new(),
            sidebar_tab_order: SidebarTab::normalize(&[]),
            sidebar_navigation_order: SidebarNavigation::ALL
                .iter()
                .map(|item| item.id().into())
                .collect(),
            sidebar_hidden_connectors: HashSet::new(),
            compact_session_cards: true,
            sidebar_notes_visible: true,
            sidebar_automations_visible: true,
            sidebar_todos_visible: true,
            sidebar_inbox_visible: true,
            sidebar_activity_visible: true,
            sidebar_subscriptions_visible: true,
            right_panel_open: false,
            right_panel_tab: Default::default(),
            right_panel_width: crate::right_panel::WIDTH_DEFAULT,
            right_panel_tabs: crate::right_panel::RightTab::normalize(&[]),
            changes_view: Default::default(),
            reviews: Default::default(),
            review_owners: Default::default(),
            layouts: Default::default(),
            focused: Default::default(),
            docks: Default::default(),
            classic_layout: true,
            preferences: crate::settings::Preferences::default(),
            session_filters: Default::default(),
            project_appearance: Default::default(),
            desktop_project_preferences: None,
            desktop_workspace_snapshot: None,
            desktop_preferences_metadata: None,
        }
    }
}
impl Workspace {
    pub fn restore(saved: Option<&str>, sessions: &[(i64, String)], cwd: &str) -> Self {
        let mut state = saved
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .and_then(|mut value| {
                vorkspace_core::desktop_preferences::DesktopPreferences::restore_sidebar_preferences(
                    &mut value,
                );
                serde_json::from_value::<Self>(value).ok()
            })
            .unwrap_or_default();
        // Recover markers retained by an earlier bridge even when the desktop
        // source is unavailable. Native decisions, including restore, win.
        if let Some(raw) = state
            .desktop_preferences_metadata
            .as_ref()
            .and_then(|m| m.get("vorkspace.archivedProjects"))
            .and_then(|v| v.as_str())
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .and_then(|v| v.as_array().cloned())
        {
            for path in raw
                .iter()
                .filter_map(|v| v.get("path").and_then(|v| v.as_str()))
                .filter(|p| !p.is_empty())
            {
                state
                    .project_archive
                    .entry(vorkspace_core::files::path_key(path))
                    .or_insert(true);
            }
        }
        let _ = state.migrate_project_rail();
        state.classic_layout = true;
        state.projects.retain(|p| {
            state
                .project_archive
                .get(&vorkspace_core::files::path_key(p))
                != Some(&true)
        });
        if state
            .project_archive
            .get(&vorkspace_core::files::path_key(&state.project))
            == Some(&true)
        {
            state.project = state.projects.first().cloned().unwrap_or_default();
            state.active = None;
        }
        if state.project.is_empty()
            && state
                .project_archive
                .get(&vorkspace_core::files::path_key(cwd))
                != Some(&true)
        {
            state.project = cwd.into();
        }
        for appearance in state.project_appearance.values_mut() {
            appearance.normalize();
        }
        let mut known: HashSet<_> = sessions.iter().map(|s| s.0).collect();
        state.review_owners.retain(|id, owner| {
            *id < 0
                && known.contains(owner)
                && state.reviews.get(id).is_some_and(|r| !r.tabs.is_empty())
        });
        known.extend(state.review_owners.keys().copied());
        let mut seen = HashSet::new();
        let mut layouts = std::mem::take(&mut state.layouts);
        let mut focused = std::mem::take(&mut state.focused);
        for tab in std::mem::take(&mut state.tabs) {
            let layout = layouts.remove(&tab).unwrap_or_else(|| leaf(tab));
            let Some(layout) = layout.retain(&mut |id| known.contains(&id) && seen.insert(id))
            else {
                continue;
            };
            let key = if layout.contains(tab) {
                tab
            } else {
                layout.first_leaf()
            };
            state.tabs.push(key);
            for entry in state
                .title_order
                .iter_mut()
                .chain(state.scope_focus.values_mut())
            {
                if *entry == TitleTab::Session(tab) {
                    *entry = TitleTab::Session(key);
                }
            }
            if layout.is_split() {
                if let Some(pane) = focused.remove(&tab).filter(|id| layout.contains(*id)) {
                    state.focused.insert(key, pane);
                }
                state.layouts.insert(key, layout);
            }
        }
        state.restore_terminal_tab_ids();
        state.normalize_title_order();
        if state
            .desktop_project_preferences
            .as_ref()
            .and_then(|source| source.get("projects"))
            .is_some()
            && state.active.is_some_and(|id| {
                sessions.iter().any(|(known, path)| {
                    *known == state.session_for_pane(id) && !state.projects.contains(path)
                })
            })
        {
            // An old native sidecar could focus an internal session folder.
            // It must not restore that folder as a remembered user project.
            state.active = None;
        }
        if state.active.is_none_or(|id| state.tab_of(id).is_none()) {
            state.active = state
                .project_focus
                .get(&state.project)
                .copied()
                .filter(|id| {
                    state.tab_of(*id).is_some()
                        && sessions.iter().any(|(known, path)| {
                            *known == state.session_for_pane(*id) && *path == state.project
                        })
                })
                .or_else(|| {
                    state
                        .tabs
                        .iter()
                        .rev()
                        .map(|tab| state.tab_focus(*tab))
                        .find(|id| {
                            sessions.iter().any(|(known, path)| {
                                *known == state.session_for_pane(*id) && *path == state.project
                            })
                        })
                });
        }
        if saved.is_none()
            && let Some((id, _)) = sessions.iter().find(|(_, path)| path == cwd)
        {
            state.open(*id);
        }
        state.sidebar_width = state.sidebar_width.clamp(200., 560.);
        state.project_rail_width = if state.project_rail_width.is_finite() {
            state.project_rail_width.clamp(180., 360.).round()
        } else {
            200.
        };
        state.sidebar_tab_order = SidebarTab::normalize(&state.sidebar_tab_order);
        state.sidebar_navigation_order =
            SidebarNavigation::normalize(&state.sidebar_navigation_order)
                .into_iter()
                .map(|item| item.id().into())
                .collect();
        state.right_panel_width = state
            .right_panel_width
            .clamp(crate::right_panel::WIDTH_MIN, crate::right_panel::WIDTH_MAX)
            .round();
        state.right_panel_tabs = crate::right_panel::RightTab::normalize(&state.right_panel_tabs);
        state
            .reviews
            .retain(|id, review| known.contains(id) && !review.tabs.is_empty());
        state.docks.retain(|_, dock| !dock.terminals.is_empty());
        for dock in state.docks.values_mut() {
            dock.active = dock.active.min(dock.terminals.len() - 1);
            dock.size = dock.side.clamp(dock.size, (4000., 4000.));
        }
        state.projects.retain(|p| {
            !state
                .deleted_projects
                .contains(&vorkspace_core::files::path_key(p))
        });
        if state
            .deleted_projects
            .contains(&vorkspace_core::files::path_key(&state.project))
        {
            state.project = state.projects.first().cloned().unwrap_or_default();
            state.active = None;
        }
        if state
            .project_archive
            .get(&vorkspace_core::files::path_key(cwd))
            != Some(&true)
            && !state
                .deleted_projects
                .contains(&vorkspace_core::files::path_key(cwd))
        {
            state.add_project(cwd);
        }
        let open: HashSet<_> = state.open_sessions().into_iter().collect();
        let review_owners = &state.review_owners;
        state.project_focus.retain(|project, id| {
            open.contains(id)
                && sessions.iter().any(|(known, path)| {
                    *known == review_owners.get(id).copied().unwrap_or(*id) && path == project
                })
        });
        if let Some((_, project)) = sessions.iter().find(|(id, path)| {
            state
                .project_archive
                .get(&vorkspace_core::files::path_key(path))
                != Some(&true)
                && !state
                    .deleted_projects
                    .contains(&vorkspace_core::files::path_key(path))
                && state
                    .active
                    .is_some_and(|pane| state.session_for_pane(pane) == *id)
        }) {
            state.project = project.clone();
            state
                .project_focus
                .insert(project.clone(), state.active.unwrap());
        }
        if !state.project.is_empty() {
            state.add_project(&state.project.clone());
        }
        state
    }
    /// Reorder the available projection, merging it into the complete order so
    /// disabled connectors retain their exact slots until they are re-enabled.
    pub fn reorder_sidebar_navigation(
        &mut self,
        source: SidebarNavigation,
        target: SidebarNavigation,
        after: bool,
        available: &[SidebarNavigation],
    ) -> bool {
        let order = SidebarNavigation::normalize(&self.sidebar_navigation_order);
        let mut visible: Vec<_> = order
            .iter()
            .copied()
            .filter(|item| available.contains(item))
            .collect();
        if source == target || !visible.contains(&source) || !visible.contains(&target) {
            return false;
        }
        visible.retain(|item| *item != source);
        let target_ix = visible.iter().position(|item| *item == target).unwrap();
        visible.insert(target_ix + usize::from(after), source);
        let mut moved = visible.into_iter();
        let next: Vec<String> = order
            .into_iter()
            .map(|item| {
                if available.contains(&item) {
                    moved.next().unwrap()
                } else {
                    item
                }
                .id()
                .into()
            })
            .collect();
        if self.sidebar_navigation_order == next {
            return false;
        }
        self.sidebar_navigation_order = next;
        true
    }

    pub fn sidebar_tab(&self) -> SidebarTab {
        self.sidebar_tabs
            .get(&self.project)
            .or_else(|| {
                self.sidebar_tabs
                    .get(&vorkspace_core::files::path_key(&self.project))
            })
            .copied()
            .unwrap_or_else(|| {
                self.sidebar_tab_order
                    .iter()
                    .find(|tab| **tab != SidebarTab::Graph)
                    .copied()
                    .unwrap_or_default()
            })
    }
    pub fn migrate_project_rail(&mut self) -> Result<(), String> {
        let Some(metadata) = &self.desktop_preferences_metadata else {
            return Ok(());
        };
        let keys = [
            "vorkspace.projectGroups",
            "vorkspace.projectGroupAssignments",
            "vorkspace.projectRailPinned",
        ];
        if self.project_rail_state.is_none() {
            if keys.iter().any(|key| metadata.get(*key).is_some()) {
                self.project_rail_state = Some(ProjectRailState::from_metadata(metadata)?);
            }
            return Ok(());
        }
        let rail = self.project_rail_state.as_mut().unwrap();
        let pending = [
            rail.awaiting_legacy_groups,
            rail.awaiting_legacy_assignments,
            rail.awaiting_legacy_pins,
        ];
        let selected: serde_json::Map<String, serde_json::Value> = keys
            .iter()
            .zip(pending)
            .filter_map(|(key, pending)| {
                pending
                    .then(|| metadata.get(*key).map(|v| ((*key).to_owned(), v.clone())))
                    .flatten()
            })
            .collect();
        if selected.is_empty() {
            return Ok(());
        }
        let legacy = ProjectRailState::from_metadata(&serde_json::Value::Object(selected))?;
        if rail.awaiting_legacy_groups && metadata.get(keys[0]).is_some() {
            let mut groups = legacy.groups;
            for group in std::mem::take(&mut rail.groups) {
                if let Some(existing) = groups.iter_mut().find(|g| g.id == group.id) {
                    *existing = group;
                } else {
                    groups.push(group);
                }
            }
            rail.groups = groups;
            rail.awaiting_legacy_groups = false;
        }
        if rail.awaiting_legacy_assignments && metadata.get(keys[1]).is_some() {
            let mut assignments = legacy.assignments;
            for (path, value) in std::mem::take(&mut rail.assignment_overrides) {
                if let Some(group) = value {
                    assignments.insert(path, group);
                } else {
                    assignments.remove(&path);
                }
            }
            rail.assignments = assignments;
            rail.awaiting_legacy_assignments = false;
        }
        if rail.awaiting_legacy_pins && metadata.get(keys[2]).is_some() {
            let mut pinned = legacy.pinned;
            for (path, value) in std::mem::take(&mut rail.pin_overrides) {
                if value {
                    pinned.insert(path);
                } else {
                    pinned.remove(&path);
                }
            }
            rail.pinned = pinned;
            rail.awaiting_legacy_pins = false;
        }
        Ok(())
    }
    pub fn project_rail_mut(&mut self) -> &mut ProjectRailState {
        self.migrate_project_rail()
            .expect("rail mutations require validated migration");
        self.project_rail_state
            .get_or_insert_with(|| ProjectRailState {
                awaiting_legacy_groups: true,
                awaiting_legacy_assignments: true,
                awaiting_legacy_pins: true,
                ..Default::default()
            })
    }
    pub fn restore_project(&mut self, path: &str) {
        self.deleted_projects
            .remove(&vorkspace_core::files::path_key(path));
        self.add_project(path);
    }
    pub fn add_project(&mut self, path: &str) {
        if path.is_empty() {
            return;
        }
        let key = vorkspace_core::files::path_key(path);
        if self.deleted_projects.contains(&key) {
            return;
        }
        if self.project_archive.contains_key(&key) {
            self.project_archive.insert(key.clone(), false);
        }
        if !self
            .projects
            .iter()
            .any(|p| vorkspace_core::files::path_key(p) == key)
        {
            self.projects.push(path.into());
        }
    }
    pub fn session_for_pane(&self, id: i64) -> i64 {
        self.review_owners.get(&id).copied().unwrap_or(id)
    }
    /// Focus a session, opening it as its own tab unless a tab holds it.
    pub fn open(&mut self, id: i64) {
        match self.tab_of(id) {
            Some(tab) => {
                if self.layouts.contains_key(&tab) {
                    self.focused.insert(tab, id);
                }
            }
            None => {
                self.tabs.push(id);
                self.title_order.push(TitleTab::Session(id));
            }
        }
        self.active = Some(id);
    }
    /// Close the whole title tab holding `id`; its sessions keep running.
    pub fn close(&mut self, id: i64) {
        let Some(tab) = self.tab_of(id) else {
            return;
        };
        let ix = self.tabs.iter().position(|t| *t == tab).unwrap_or(0);
        let layout = self.layout(tab);
        self.tabs.remove(ix);
        self.tab_workspaces.remove(&tab);
        self.title_order
            .retain(|entry| *entry != TitleTab::Session(tab));
        self.scope_focus
            .retain(|_, entry| *entry != TitleTab::Session(tab));
        self.layouts.remove(&tab);
        self.focused.remove(&tab);
        self.project_focus.retain(|_, id| !layout.contains(*id));
        if self
            .active
            .is_some_and(|active| self.tab_of(active).is_none())
        {
            self.active = self
                .tabs
                .get(ix)
                .or_else(|| self.tabs.last())
                .map(|tab| self.tab_focus(*tab));
        }
    }
    /// The title tab holding a session.
    pub fn tab_of(&self, id: i64) -> Option<i64> {
        self.tabs.iter().copied().find(|tab| {
            *tab == id
                || self
                    .layouts
                    .get(tab)
                    .is_some_and(|layout| layout.contains(id))
        })
    }
    pub fn active_tab(&self) -> Option<i64> {
        self.active.and_then(|id| self.tab_of(id))
    }
    pub fn layout(&self, tab: i64) -> PaneNode {
        self.layouts.get(&tab).cloned().unwrap_or_else(|| leaf(tab))
    }
    /// Every session shown in some tab.
    pub fn open_sessions(&self) -> Vec<i64> {
        self.tabs
            .iter()
            .flat_map(|tab| self.layout(*tab).leaf_ids())
            .collect()
    }
    /// The pane a tab activates.
    pub fn tab_focus(&self, tab: i64) -> i64 {
        self.focused
            .get(&tab)
            .copied()
            .filter(|id| self.layouts.get(&tab).is_some_and(|l| l.contains(*id)))
            .unwrap_or(tab)
    }
    /// Store a tab's tree, rekeying it when its key session left and
    /// dropping the tree once a single pane remains.
    fn set_layout(&mut self, tab: i64, layout: PaneNode) -> i64 {
        let Some(ix) = self.tabs.iter().position(|t| *t == tab) else {
            return tab;
        };
        let focus = self.focused.remove(&tab);
        self.layouts.remove(&tab);
        let key = if layout.contains(tab) {
            tab
        } else {
            layout.first_leaf()
        };
        self.tabs[ix] = key;
        if key != tab
            && let Some(scope) = self.tab_workspaces.remove(&tab)
        {
            self.tab_workspaces.insert(key, scope);
        }
        for entry in self
            .title_order
            .iter_mut()
            .chain(self.scope_focus.values_mut())
        {
            if *entry == TitleTab::Session(tab) {
                *entry = TitleTab::Session(key);
            }
        }
        if layout.is_split() {
            if let Some(focus) = focus.filter(|id| layout.contains(*id)) {
                self.focused.insert(key, focus);
            }
            self.layouts.insert(key, layout);
        }
        key
    }
    /// Take a session out of whichever tab holds it. A tab left empty
    /// closes; the active session moves to a neighbouring pane.
    fn take_out(&mut self, id: i64) {
        let Some(tab) = self.tab_of(id) else {
            return;
        };
        let layout = self.layout(tab);
        self.project_focus.retain(|_, focus| *focus != id);
        match layout.remove(id) {
            Some(rest) => {
                let next = layout.sibling(id).unwrap_or_else(|| rest.first_leaf());
                let key = self.set_layout(tab, rest);
                if self.active == Some(id) {
                    self.active = Some(next);
                }
                if self.layouts.contains_key(&key) && self.focused.get(&key) == Some(&id) {
                    self.focused.insert(key, next);
                }
            }
            None => self.close(id),
        }
    }
    /// Close one pane of a split tab. False when it is the tab's only pane.
    pub fn close_pane(&mut self, id: i64) -> bool {
        let Some(tab) = self.tab_of(id) else {
            return false;
        };
        if !self.layout(tab).is_split() {
            return false;
        }
        self.take_out(id);
        if let Some(active) = self.active
            && let Some(tab) = self.tab_of(active)
            && self.layouts.contains_key(&tab)
        {
            self.focused.insert(tab, active);
        }
        true
    }
    /// cmd-d: a new session beside the focused pane.
    pub fn split(&mut self, new: i64, dir: SplitDir) {
        let Some(focused) = self.active else {
            self.open(new);
            return;
        };
        let Some(tab) = self.tab_of(focused) else {
            self.open(new);
            return;
        };
        self.take_out(new);
        let tab = self.tab_of(focused).unwrap_or(tab);
        let key = self.set_layout(tab, self.layout(tab).split_pane(focused, dir, new));
        self.focused.insert(key, new);
        self.active = Some(new);
    }
    /// A pane, tab-strip session or sidebar session dropped on a pane edge.
    pub fn place(&mut self, id: i64, target: i64, edge: Edge) {
        if id == target || self.tab_of(target).is_none() {
            return;
        }
        let same = self.tab_of(id) == self.tab_of(target);
        if !same {
            self.take_out(id);
        }
        let Some(tab) = self.tab_of(target) else {
            return;
        };
        let key = self.set_layout(tab, self.layout(tab).place_pane(id, target, edge));
        self.focused.insert(key, id);
        self.active = Some(id);
    }
    /// A whole title tab dropped on a pane edge: its tree moves intact.
    pub fn place_tab(&mut self, source: i64, target: i64, edge: Edge) {
        let Some(target_tab) = self.tab_of(target) else {
            return;
        };
        if source == target_tab || !self.tabs.contains(&source) {
            return;
        }
        let incoming = self.layout(source);
        let focus = self.tab_focus(source);
        self.tabs.retain(|t| *t != source);
        self.tab_workspaces.remove(&source);
        self.title_order
            .retain(|entry| *entry != TitleTab::Session(source));
        self.layouts.remove(&source);
        self.focused.remove(&source);
        let key = self.set_layout(
            target_tab,
            self.layout(target_tab).place_layout(incoming, target, edge),
        );
        self.focused.insert(key, focus);
        self.active = Some(focus);
    }
    /// A pane dragged onto the tab strip becomes its own tab at `to`.
    pub fn detach(&mut self, id: i64, to: usize) {
        if !self
            .tab_of(id)
            .is_some_and(|tab| self.layout(tab).is_split())
        {
            return;
        }
        let scope = self
            .tab_of(id)
            .and_then(|tab| self.tab_workspaces.get(&tab))
            .cloned();
        self.take_out(id);
        self.tabs.insert(to.min(self.tabs.len()), id);
        if let Some(scope) = scope {
            self.tab_workspaces.insert(id, scope);
        }
        let next = self
            .tabs
            .iter()
            .skip(to + 1)
            .find_map(|next| {
                self.title_order
                    .iter()
                    .position(|tab| *tab == TitleTab::Session(*next))
            })
            .unwrap_or(self.title_order.len());
        self.title_order.insert(next, TitleTab::Session(id));
        self.active = Some(id);
    }
    /// Show `to` in `from`'s pane (Session: Next/Previous in tab).
    pub fn swap_session(&mut self, from: i64, to: i64) {
        if self.tab_of(to).is_some() {
            return;
        }
        let Some(tab) = self.tab_of(from) else {
            return;
        };
        let key = self.set_layout(tab, self.layout(tab).replace_leaf(from, to));
        if self.layouts.contains_key(&key) {
            self.focused.insert(key, to);
        }
    }
    /// cmd-opt-arrows within the active tab.
    pub fn neighbor(&self, dir: Edge) -> Option<i64> {
        let active = self.active?;
        self.layout(self.tab_of(active)?).neighbor(active, dir)
    }
    pub fn place_sash(&mut self, tab: i64, path: &[usize], index: usize, boundary: f32) {
        if let Some(layout) = self.layouts.get_mut(&tab) {
            layout.place_sash(path, index, boundary);
        }
    }
    pub fn sash_range(&self, tab: i64, path: &[usize], index: usize) -> Option<(f32, f32, f32)> {
        self.layouts.get(&tab)?.sash_range(path, index)
    }
    pub fn new_terminal_tab(&self, cwd: String) -> TitleTab {
        TitleTab::Terminal {
            id: fresh_terminal_tab_id(&self.terminal_tab_ids()),
            cwd,
        }
    }

    fn terminal_tab_ids(&self) -> HashSet<u64> {
        self.title_order
            .iter()
            .chain(self.scope_focus.values())
            .filter_map(|tab| match tab {
                TitleTab::Terminal { id, .. } => Some(*id),
                _ => None,
            })
            .collect()
    }

    /// Upgrade only legacy terminal entries, before title-order deduplication.
    /// Scope references must resolve to the same identity as their saved tab.
    fn restore_terminal_tab_ids(&mut self) {
        let mut used = self.terminal_tab_ids();
        // Reserve even lazily restored IDs before any of these tabs can close.
        // A delayed prompt for a closed saved tab must never target a new admission.
        if let Some(max) = used.iter().max() {
            NEXT_TERMINAL_TAB_ID.fetch_max(max.saturating_add(1), Ordering::Relaxed);
        }
        let mut legacy = HashMap::new();
        for tab in &mut self.title_order {
            if let TitleTab::Terminal { id, cwd } = tab
                && *id == 0
            {
                *id = fresh_terminal_tab_id(&used);
                used.insert(*id);
                legacy.entry(cwd.clone()).or_insert(*id);
            }
        }
        for tab in self.scope_focus.values_mut() {
            if let TitleTab::Terminal { id, cwd } = tab
                && *id == 0
            {
                *id = *legacy.entry(cwd.clone()).or_insert_with(|| {
                    let id = fresh_terminal_tab_id(&used);
                    used.insert(id);
                    id
                });
            }
        }
    }

    pub fn normalize_title_order(&mut self) {
        self.tab_workspaces.retain(|tab, _| self.tabs.contains(tab));
        let mut seen = HashSet::new();
        self.title_order.retain(|tab| {
            tab.in_strip()
                && tab.session().is_none_or(|id| self.tabs.contains(&id))
                && seen.insert(tab.clone())
        });
        self.scope_focus.retain(|_, tab| tab.in_strip());
        for id in &self.tabs {
            let tab = TitleTab::Session(*id);
            if seen.insert(tab.clone()) {
                self.title_order.push(tab);
            }
        }
    }
}

/// Never reuse an admission identity in this process, even after its tab is
/// closed with a pending prompt. Skip saved IDs when reopening a workspace.
fn fresh_terminal_tab_id(used: &HashSet<u64>) -> u64 {
    loop {
        let id = NEXT_TERMINAL_TAB_ID.fetch_add(1, Ordering::Relaxed);
        if id != 0 && !used.contains(&id) {
            return id;
        }
    }
}

/// Port of src/features/workspace/model/tabVisitHistory.ts. Closed tabs are
/// pruned rather than reopened by Back, and a new visit discards Forward.
#[derive(Default)]
pub(crate) struct VisitHistory {
    pub back: Vec<i64>,
    pub forward: Vec<i64>,
    current: Option<i64>,
}
impl VisitHistory {
    fn push(&mut self, id: i64) {
        if self.back.last() != Some(&id) {
            self.back.push(id);
        }
        if self.back.len() > 50 {
            self.back.remove(0);
        }
    }
    pub fn record(&mut self, id: i64) {
        if self.current == Some(id) {
            return;
        }
        if let Some(current) = self.current {
            self.push(current);
        }
        self.current = Some(id);
        self.forward.clear();
    }
    pub fn step(&mut self, direction: isize) -> Option<i64> {
        let target = if direction < 0 {
            let target = self.back.pop()?;
            if let Some(current) = self.current {
                self.forward.insert(0, current);
            }
            target
        } else {
            if self.forward.is_empty() {
                return None;
            }
            let target = self.forward.remove(0);
            if let Some(current) = self.current {
                self.push(current);
            }
            target
        };
        self.current = Some(target);
        Some(target)
    }
    pub fn prune(&mut self, tabs: &[i64], active: Option<i64>) {
        self.back.retain(|id| tabs.contains(id));
        self.forward.retain(|id| tabs.contains(id));
        if self.current.is_none_or(|id| !tabs.contains(&id)) {
            self.current = active;
        }
        while self
            .back
            .last()
            .copied()
            .is_some_and(|id| Some(id) == self.current)
        {
            self.back.pop();
        }
        while self
            .forward
            .first()
            .copied()
            .is_some_and(|id| Some(id) == self.current)
        {
            self.forward.remove(0);
        }
    }
}
pub(crate) fn project_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .filter(|n| !n.is_empty())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.into())
}

#[cfg(test)]
mod tests;
