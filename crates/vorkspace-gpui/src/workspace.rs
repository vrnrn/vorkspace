//! Viewport state: closing a tab never deletes or stops a session.
//! A title tab is keyed by one of its sessions; a tab holding more than one
//! pane keeps its split tree in `layouts` and its last focused pane in
//! `focused`. `active` is always the focused session.
use crate::pane_layout::{Edge, PaneNode, SplitDir, leaf};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

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
}
impl AppView {
    pub fn label(self) -> &'static str {
        match self {
            Self::Inbox => "Connectors",
            Self::Connector(provider) => provider.label(),
            Self::Notes => "Notes",
            Self::Automations => "Automations",
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            Self::Inbox => "globe",
            Self::Connector(provider) => provider.icon(),
            Self::Notes => "note",
            Self::Automations => "zap",
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
    Terminal,
}
impl SidebarNavigation {
    pub const ALL: [Self; 6] = [
        Self::Connector(Connector::Github),
        Self::Connector(Connector::Gitlab),
        Self::Connector(Connector::Jira),
        Self::Notes,
        Self::Automations,
        Self::Terminal,
    ];
    pub fn id(self) -> &'static str {
        match self {
            Self::Connector(provider) => provider.id(),
            Self::Notes => "notes",
            Self::Automations => "automations",
            Self::Terminal => "terminal",
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
            Self::Terminal => "Terminal",
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            Self::Connector(provider) => provider.icon(),
            Self::Notes => "note",
            Self::Automations => "zap",
            Self::Terminal => "terminal",
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

/// The title strip has one order, including conversations, app views and Bots.
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
}
impl SidebarTab {
    pub fn normalize(order: &[Self]) -> Vec<Self> {
        let mut tabs = Vec::new();
        for tab in order
            .iter()
            .chain(&[Self::Sessions, Self::Files, Self::Changes])
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
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            Self::Sessions => "message-multiple",
            Self::Files => "folder",
            Self::Changes => "git-compare",
            Self::Graph => "git-branch",
        }
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
    pub worktree_focus: HashMap<String, crate::worktree_switcher::WorktreeFocus>,
    /// Directly opened chats join the workspace on screen without changing
    /// their execution directory. Like workspace selection, these pins are local
    /// to the current run and disappear when their tabs close.
    #[serde(skip)]
    pub tab_workspaces: HashMap<i64, String>,
    pub active: Option<i64>,
    pub projects: Vec<String>,
    /// Explicit decisions keyed by normalized path; false prevents stale legacy re-import.
    pub project_archive: HashMap<String, bool>,
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
    pub sidebar_terminal_visible: bool,
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
    pub changes_view: crate::changes::ChangesView,
    /// Each tab's file/review pane beside its chat.
    pub reviews: std::collections::HashMap<i64, crate::review::ReviewState>,
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
            project: String::new(),
            project_focus: HashMap::new(),
            recent_files: HashMap::new(),
            collapsed: HashSet::new(),
            sidebar_visible: true,
            sidebar_width: 260.,
            project_rail_open: true,
            project_rail_width: 200.,
            project_group_collapsed: HashMap::new(),
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
            sidebar_terminal_visible: true,
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
        if state
            .project_archive
            .get(&vorkspace_core::files::path_key(cwd))
            != Some(&true)
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

    /// The old Inbox visibility flag is deliberately ignored: enable toggles
    /// are the sole authority for connector availability.
    pub fn normalize_connector_tabs(&mut self, state: &vorkspace_core::inbox::ViewState) {
        let fallback = Connector::from_id(&state.source)
            .filter(|provider| state.connectors.enabled(provider.id()))
            .or_else(|| {
                state
                    .connectors
                    .enabled_providers()
                    .find_map(Connector::from_id)
            });
        let normalize = |tab: &TitleTab| match tab {
            TitleTab::App(AppView::Inbox) => {
                fallback.map(|provider| TitleTab::App(AppView::Connector(provider)))
            }
            TitleTab::App(AppView::Connector(provider))
                if !state.connectors.enabled(provider.id()) =>
            {
                None
            }
            _ => Some(tab.clone()),
        };
        self.title_order = self.title_order.iter().filter_map(normalize).collect();
        self.scope_focus = self
            .scope_focus
            .iter()
            .filter_map(|(scope, tab)| normalize(tab).map(|tab| (scope.clone(), tab)))
            .collect();
        self.normalize_title_order();
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
    pub fn add_project(&mut self, path: &str) {
        if path.is_empty() {
            return;
        }
        let key = vorkspace_core::files::path_key(path);
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
            tab.session().is_none_or(|id| self.tabs.contains(&id)) && seen.insert(tab.clone())
        });
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
mod tests {
    use super::*;

    #[test]
    fn archived_project_is_not_reopened_by_active_session_or_stale_metadata() {
        let saved = serde_json::json!({
            "projects": ["/work/old", "/work/other"], "project": "/work/old", "active": 42,
            "tabs": [42], "project_archive": {"/work/old": true},
            "desktop_preferences_metadata": {"vorkspace.archivedProjects": "[{\"path\":\"/work/old/\"}]"}
        }).to_string();
        let mut state = Workspace::restore(Some(&saved), &[(42, "/work/old".into())], "/work/old");
        assert_eq!(state.project, "/work/other");
        assert!(!state.projects.contains(&"/work/old".into()));
        assert!(state.tabs.contains(&42));
        state.add_project("/work/old/");
        let saved = serde_json::to_string(&state).unwrap();
        let state = Workspace::restore(Some(&saved), &[(42, "/work/old".into())], "/work/other");
        assert_eq!(state.project_archive.get("/work/old"), Some(&false));
        assert!(
            state
                .projects
                .iter()
                .any(|p| vorkspace_core::files::path_key(p) == "/work/old")
        );
    }

    #[test]
    fn terminal_legacy_restore_preserves_other_tabs_and_upgrades_scope_identity() {
        let raw = r#"{"tabs":[42],"active":42,"project":"/work","title_order":[{"Session":42},{"terminal":{"cwd":"/work"}},{"App":"Notes"},{"terminal":{"cwd":"/work"}},{"terminal":{"id":700,"cwd":"/work"}},{"terminal":{"cwd":"/other"}},"Search",{"Bot":{"id":"bot","project":"/work","worktree":"/work"}}],"scope_focus":{"first":{"terminal":{"cwd":"/work"}},"also-first":{"terminal":{"cwd":"/work"}},"other":{"terminal":{"cwd":"/other"}},"modern":{"terminal":{"id":700,"cwd":"/work"}}}}"#;
        let workspace = Workspace::restore(Some(raw), &[(42, "/work".into())], "/work");
        assert_eq!(workspace.title_order.len(), 8);
        assert_eq!(workspace.title_order[0], TitleTab::Session(42));
        assert_eq!(workspace.title_order[2], TitleTab::App(AppView::Notes));
        assert_eq!(workspace.title_order[6], TitleTab::Search);
        assert!(matches!(&workspace.title_order[7], TitleTab::Bot { id, .. } if id == "bot"));
        assert_eq!(workspace.active, Some(42));
        let terminals: Vec<_> = workspace
            .title_order
            .iter()
            .filter_map(|tab| match tab {
                TitleTab::Terminal { id, cwd } => Some((*id, cwd.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(terminals.len(), 4);
        assert_eq!(
            terminals
                .iter()
                .map(|(id, _)| *id)
                .collect::<HashSet<_>>()
                .len(),
            4
        );
        assert!(terminals.iter().all(|(id, _)| *id != 0));
        assert_eq!(
            terminals.iter().filter(|(_, cwd)| *cwd == "/work").count(),
            3
        );
        assert_eq!(workspace.scope_focus["first"], workspace.title_order[1]);
        assert_eq!(
            workspace.scope_focus["also-first"],
            workspace.title_order[1]
        );
        assert_eq!(workspace.scope_focus["other"], workspace.title_order[5]);
        assert_eq!(workspace.scope_focus["modern"], workspace.title_order[4]);
        assert!(matches!(
            workspace.title_order[4],
            TitleTab::Terminal { id: 700, .. }
        ));
        let saved = serde_json::to_string(&workspace).unwrap();
        let reopened = Workspace::restore(Some(&saved), &[(42, "/work".into())], "/work");
        assert_eq!(reopened.title_order, workspace.title_order);
        assert_eq!(reopened.scope_focus, workspace.scope_focus);
        let TitleTab::Terminal { id, .. } = reopened.new_terminal_tab("/work".into()) else {
            unreachable!()
        };
        assert!(!terminals.iter().any(|(used, _)| *used == id));
        assert!(id > 700, "saved identities stay reserved after closing");
    }

    #[test]
    fn connector_legacy_restore_preserves_other_tabs_and_uses_enabled_saved_source() {
        let raw = r#"{"tabs":[1],"active":1,"project":"/work","title_order":[{"Session":1},{"App":"Inbox"},{"App":"Notes"},{"App":{"Connector":"gitlab"}}],"scope_focus":{"scope":{"App":"Inbox"}},"sidebar_inbox_visible":false}"#;
        let mut state = vorkspace_core::inbox::ViewState {
            source: "gitlab".into(),
            ..Default::default()
        };
        let mut workspace = Workspace::restore(Some(raw), &[(1, "/work".into())], "/work");
        workspace.normalize_connector_tabs(&state);
        assert_eq!(
            workspace.title_order,
            [
                TitleTab::Session(1),
                TitleTab::App(AppView::Connector(Connector::Gitlab)),
                TitleTab::App(AppView::Notes)
            ]
        );
        assert_eq!(
            workspace.scope_focus["scope"],
            TitleTab::App(AppView::Connector(Connector::Gitlab))
        );
        state.connectors.set("gitlab", false).unwrap();
        let mut fallback = Workspace::restore(Some(raw), &[(1, "/work".into())], "/work");
        fallback.normalize_connector_tabs(&state);
        assert!(
            fallback
                .title_order
                .contains(&TitleTab::App(AppView::Connector(Connector::Github)))
        );
        for provider in vorkspace_core::inbox::PROVIDERS {
            state.connectors.set(provider, false).unwrap();
        }
        fallback.normalize_connector_tabs(&state);
        assert_eq!(
            fallback.title_order,
            [TitleTab::Session(1), TitleTab::App(AppView::Notes)]
        );
        let encoded = serde_json::to_string(&workspace).unwrap();
        let restored = Workspace::restore(Some(&encoded), &[(1, "/work".into())], "/work");
        assert_eq!(restored.title_order, workspace.title_order);
        assert_eq!(
            serde_json::from_str::<crate::settings::Section>(r#""inbox""#).unwrap(),
            crate::settings::Section::Connectors
        );
    }
    #[test]
    fn original_layout_is_used_for_fresh_and_previously_experimental_workspaces() {
        assert!(Workspace::default().classic_layout);
        assert!(
            serde_json::from_str::<Workspace>("{}")
                .unwrap()
                .classic_layout
        );
        for classic in [false, true] {
            let saved =
                serde_json::json!({"classic_layout":classic,"new_layout_prompt_dismissed":false});
            let restored = Workspace::restore(Some(&saved.to_string()), &[], "/work");
            assert!(restored.classic_layout);
            let reopened = Workspace::restore(
                Some(&serde_json::to_string(&restored).unwrap()),
                &[],
                "/work",
            );
            assert!(reopened.classic_layout);
        }
    }
    #[test]
    fn sidebar_navigation_normalizes_ids_and_preserves_disabled_slots_and_hidden_state() {
        let saved = serde_json::json!({
            "sidebar_navigation_order": ["notes", "unknown", "github", "notes", "automations"],
            "sidebar_hidden_connectors": ["github"],
            "compact_session_cards": false,
            "sidebar_activity_visible": true,
        });
        let mut state = Workspace::restore(Some(&saved.to_string()), &[], "/work");
        assert_eq!(
            state.sidebar_navigation_order,
            [
                "notes",
                "github",
                "automations",
                "gitlab",
                "jira",
                "terminal"
            ]
        );
        use SidebarNavigation::{Automations, Connector as Provider, Notes};
        let available = [
            Provider(Connector::Github),
            Provider(Connector::Jira),
            Notes,
            Automations,
        ];
        assert!(state.reorder_sidebar_navigation(
            Provider(Connector::Jira),
            Notes,
            false,
            &available
        ));
        assert_eq!(
            state.sidebar_navigation_order,
            [
                "jira",
                "notes",
                "github",
                "gitlab",
                "automations",
                "terminal"
            ]
        );
        // GitLab's slot survives its absence; hidden GitHub is still reorderable.
        assert!(state.reorder_sidebar_navigation(
            Provider(Connector::Github),
            Automations,
            true,
            &available
        ));
        assert_eq!(
            state.sidebar_navigation_order,
            [
                "jira",
                "notes",
                "automations",
                "gitlab",
                "github",
                "terminal"
            ]
        );
        assert!(!state.reorder_sidebar_navigation(
            Provider(Connector::Gitlab),
            Notes,
            false,
            &available
        ));
        assert!(!state.reorder_sidebar_navigation(Notes, Notes, true, &available));
        let restored =
            Workspace::restore(Some(&serde_json::to_string(&state).unwrap()), &[], "/work");
        assert_eq!(
            restored.sidebar_navigation_order,
            state.sidebar_navigation_order
        );
        assert_eq!(
            restored.sidebar_hidden_connectors,
            state.sidebar_hidden_connectors
        );
        assert!(!restored.compact_session_cards);
        assert_eq!(SidebarNavigation::normalize(&[]), SidebarNavigation::ALL);
        assert!(state.reorder_sidebar_navigation(
            Provider(Connector::Gitlab),
            Notes,
            false,
            &SidebarNavigation::ALL
        ));
        assert_eq!(
            state.sidebar_navigation_order,
            [
                "jira",
                "gitlab",
                "notes",
                "automations",
                "github",
                "terminal"
            ]
        );
    }

    #[test]
    fn sidebar_tools_keep_per_project_selection_and_normalize_main_tab_order() {
        assert_eq!(
            SidebarTab::normalize(&[SidebarTab::Changes, SidebarTab::Graph, SidebarTab::Changes]),
            [SidebarTab::Changes, SidebarTab::Sessions, SidebarTab::Files]
        );
        let saved = serde_json::json!({
            "project":"/work",
            "project_rail_open":false,
            "project_rail_width":900.,
            "sidebar_tabs":{"/work":"graph","/other":"files"},
            "sidebar_tab_order":["changes","graph","changes"]
        });
        let mut state = Workspace::restore(Some(&saved.to_string()), &[], "/work");
        assert_eq!(state.sidebar_tab(), SidebarTab::Graph);
        assert!(!state.project_rail_open);
        assert_eq!(state.project_rail_width, 360.);
        assert_eq!(
            state.sidebar_tab_order,
            [SidebarTab::Changes, SidebarTab::Sessions, SidebarTab::Files]
        );
        state.project = "/other".into();
        assert_eq!(state.sidebar_tab(), SidebarTab::Files);
        state
            .sidebar_tabs
            .insert("/other".into(), SidebarTab::Sessions);
        state.project_rail_width = 225.;
        let reopened =
            Workspace::restore(Some(&serde_json::to_string(&state).unwrap()), &[], "/other");
        assert_eq!(reopened.sidebar_tab(), SidebarTab::Sessions);
        assert_eq!(reopened.project_rail_width, 225.);
        state.project = "/new".into();
        assert_eq!(state.sidebar_tab(), SidebarTab::Changes);
    }
    #[test]
    fn earlier_native_save_recovers_original_sidebar_from_retained_desktop_metadata() {
        let saved = serde_json::json!({
            "project":"/work",
            "preferences":{"sounds":false},
            "desktop_preferences_metadata":{
                "vorkspace.projectRailOpen":"0",
                "vorkspace.projectRailWidth":"246",
                "vorkspace.projectSidebarTabs.v1":r#"{"/work":"files"}"#,
                "vorkspace.sidebarTabOrder":r#"["changes","inbox","sessions"]"#
            }
        });
        let state = Workspace::restore(Some(&saved.to_string()), &[], "/work");
        assert!(!state.project_rail_open);
        assert_eq!(state.project_rail_width, 246.);
        assert_eq!(state.sidebar_tab(), SidebarTab::Files);
        assert!(!state.preferences.sounds);
        assert_eq!(
            serde_json::to_value(state.desktop_preferences_metadata).unwrap(),
            saved["desktop_preferences_metadata"]
        );
        let malformed = serde_json::json!({
            "project":"/work",
            "project_rail_open":"invalid",
            "project_rail_width":-20.,
            "sidebar_tabs":{"/work":"files","/bad":7},
            "sidebar_tab_order":["changes",null,"graph"]
        });
        let restored = Workspace::restore(Some(&malformed.to_string()), &[], "/work");
        assert_eq!(restored.project, "/work");
        assert!(restored.project_rail_open);
        assert_eq!(restored.project_rail_width, 180.);
        assert_eq!(restored.sidebar_tab(), SidebarTab::Files);
    }
    #[test]
    fn navigation_prunes_closed_tabs_and_a_new_visit_discards_forward() {
        let mut h = VisitHistory::default();
        h.record(1);
        h.record(2);
        h.record(3);
        assert_eq!(h.step(-1), Some(2));
        h.prune(&[1, 3], Some(3));
        assert_eq!(h.step(-1), Some(1));
        assert_eq!(h.step(1), Some(3));
        h.step(-1);
        h.record(4);
        assert!(h.forward.is_empty());
        for id in 5..100 {
            h.record(id);
        }
        assert_eq!(h.back.len(), 50);
    }
    #[test]
    fn close_and_reopen_keep_session_identity() {
        let mut s = Workspace::default();
        s.open(1);
        s.open(2);
        s.open(3);
        s.open(2);
        assert_eq!(s.tabs, vec![1, 2, 3]);
        s.close(1);
        assert_eq!(s.active, Some(2));
        s.close(2);
        assert_eq!(s.active, Some(3));
        s.open(2);
        s.open(3);
        assert_eq!(s.active, Some(3));
        s.close(3);
        s.close(2);
        assert_eq!(s.active, None);
        s.open(1);
        assert_eq!(s.tabs, vec![1]);
    }
    #[test]
    fn split_tabs_place_detach_and_close_panes_without_losing_sessions() {
        let mut s = Workspace::default();
        s.open(1);
        s.open(2);
        s.open(1);
        s.split(3, SplitDir::Right);
        s.tab_workspaces.insert(1, "/selected".into());
        s.tab_workspaces.insert(2, "/other".into());
        assert_eq!(s.tabs, [1, 2]);
        assert_eq!(s.layout(1).leaf_ids(), [1, 3]);
        assert_eq!((s.active, s.active_tab()), (Some(3), Some(1)));
        // Another tab's session joins this one; its old tab closes.
        s.place(2, 1, Edge::Bottom);
        assert_eq!(s.tabs, [1]);
        assert!(!s.tab_workspaces.contains_key(&2));
        assert_eq!(s.layout(1).leaf_ids(), [1, 2, 3]);
        assert_eq!(s.open_sessions(), [1, 2, 3]);
        // Closing the key pane rekeys the tab and keeps its place.
        s.open(4);
        s.open(1);
        assert!(s.close_pane(1));
        assert_eq!(s.tabs, [2, 4]);
        assert_eq!(s.tab_workspaces[&2], "/selected");
        assert!(!s.tab_workspaces.contains_key(&1));
        assert_eq!(s.active, Some(2));
        assert_eq!(s.tab_focus(2), 2);
        s.open(3);
        s.open(s.tab_focus(4));
        assert_eq!(s.active, Some(4));
        s.open(s.tab_focus(2));
        assert_eq!(s.active, Some(3));
        // A pane dragged to the strip becomes a tab where it was dropped.
        s.detach(3, 0);
        assert_eq!(s.tabs, [3, 2, 4]);
        assert_eq!(s.tab_workspaces[&3], "/selected");
        assert!(!s.layouts.contains_key(&2));
        assert!(!s.close_pane(3));
        // A whole tab dropped on a pane moves in intact.
        s.split(5, SplitDir::Down);
        s.place_tab(3, 4, Edge::Left);
        assert_eq!(s.tabs, [2, 4]);
        assert!(!s.tab_workspaces.contains_key(&3));
        assert_eq!(s.layout(4).leaf_ids(), [3, 5, 4]);
        assert_eq!(s.active, Some(5));
        s.close(5);
        assert_eq!(s.tabs, [2]);
        assert_eq!(s.tab_workspaces.len(), 1);
        assert_eq!(s.active, Some(2));
    }
    #[test]
    fn restore_keeps_split_tabs_and_prunes_their_missing_panes() {
        let mut s = Workspace::default();
        s.open(1);
        s.split(2, SplitDir::Right);
        s.split(3, SplitDir::Down);
        s.open(9);
        s.open(2);
        let json = serde_json::to_string(&s).unwrap();
        let sessions: Vec<_> = [2, 3].iter().map(|id| (*id, "/w".into())).collect();
        let s = Workspace::restore(Some(&json), &sessions, "/w");
        assert_eq!(s.tabs, [2]);
        assert_eq!(s.layout(2).leaf_ids(), [2, 3]);
        assert_eq!(s.active, Some(2));
        assert_eq!(s.tab_focus(2), 2);
    }
    #[test]
    fn terminal_docks_add_close_move_and_clamp_like_project_terminal() {
        let mut dock = TerminalDock::default();
        assert_eq!((dock.side, dock.size), (DockSide::Bottom, 220.));
        dock.add("/w/a".into());
        dock.add("/w/b".into());
        dock.add("/w/c".into());
        assert_eq!(dock.active, 2);
        dock.active = 1;
        assert!(dock.close(1));
        assert_eq!((dock.terminals.len(), dock.active), (2, 1));
        assert!(dock.close(0));
        assert_eq!(dock.active, 0);
        assert!(!dock.close(0));
        dock.set_side(DockSide::Right);
        assert_eq!(dock.size, 360.);
        dock.size = 500.;
        dock.set_side(DockSide::Left);
        assert_eq!(dock.size, 500.);
        assert_eq!(DockSide::Bottom.clamp(10., (1280., 800.)), 88.);
        assert_eq!(DockSide::Bottom.clamp(900., (1280., 800.)), 560.);
        assert_eq!(DockSide::Left.clamp(f32::NAN, (1280., 800.)), 360.);
        let mut s = Workspace::default();
        s.docks.insert("/w".into(), dock.clone());
        let mut kept = dock.clone();
        kept.add("/w".into());
        s.docks.insert("/k".into(), kept);
        let json = serde_json::to_string(&s).unwrap();
        let s = Workspace::restore(Some(&json), &[], "/w");
        assert!(!s.docks.contains_key("/w"));
        assert_eq!(s.docks["/k"].terminals, ["/w"]);
    }
    #[test]
    fn terminal_docks_reorder_keeping_the_active_terminal_and_close_others() {
        let mut dock = TerminalDock::default();
        for cwd in ["/a", "/b", "/c", "/d"] {
            dock.add(cwd.into());
        }
        dock.active = 1;
        dock.reorder(1, 3);
        assert_eq!(dock.terminals, ["/a", "/c", "/d", "/b"]);
        assert_eq!(dock.active, 3);
        dock.reorder(0, 2);
        assert_eq!(dock.terminals, ["/c", "/d", "/a", "/b"]);
        assert_eq!(dock.active, 3);
        dock.reorder(3, 0);
        assert_eq!(dock.terminals, ["/b", "/c", "/d", "/a"]);
        assert_eq!(dock.active, 0);
        dock.active = 2;
        dock.reorder(3, 1);
        assert_eq!(dock.terminals, ["/b", "/a", "/c", "/d"]);
        assert_eq!(dock.active, 3);
        dock.keep_only(2);
        assert_eq!(
            (dock.terminals.clone(), dock.active),
            (vec!["/c".to_string()], 0)
        );
    }
    #[test]
    fn restore_keeps_project_selection_and_prunes_stale_project_focus() {
        let sessions = vec![(3, "/b".into()), (2, "/a".into()), (1, "/a".into())];
        let mut state = Workspace::default();
        for id in [1, 2, 3] {
            state.open(id);
        }
        state.project = "/a".into();
        state.active = Some(99);
        state.project_focus.insert("/a".into(), 1);
        state.project_focus.insert("/b".into(), 99);
        state.worktree_focus.insert(
            "/a".into(),
            crate::worktree_switcher::WorktreeFocus {
                path: "/a-tree".into(),
                branch: Some("feature".into()),
            },
        );
        state.tab_workspaces.insert(1, "/a-tree".into());
        let mut saved = serde_json::to_value(&state).unwrap();
        assert!(saved.get("worktree_focus").is_none());
        assert!(saved.get("tab_workspaces").is_none());
        // Older native sidecars saved this transient filter. Ignore it too.
        saved["worktree_focus"] = serde_json::json!({
            "/a": {"path":"/a-tree", "branch":"feature"}
        });
        let json = serde_json::to_string(&saved).unwrap();
        let restored = Workspace::restore(Some(&json), &sessions, "/b");
        assert_eq!(restored.active, Some(1));
        assert_eq!(restored.project, "/a");
        assert!(restored.worktree_focus.is_empty());
        assert!(restored.tab_workspaces.is_empty());
        assert!(!restored.project_focus.contains_key("/b"));
        state.active = Some(3);
        let json = serde_json::to_string(&state).unwrap();
        let restored = Workspace::restore(Some(&json), &sessions, "/a");
        assert_eq!(restored.active, Some(3));
        assert_eq!(restored.project, "/b");
        let fresh = Workspace::restore(None, &sessions, "/a");
        assert_eq!(fresh.active, Some(2));
        assert_eq!(fresh.project, "/a");
    }
    #[test]
    fn restoring_history_keeps_only_remembered_or_selected_projects() {
        let saved = serde_json::json!({
            "project": "/code/app",
            "projects": ["/code/app", "/code/other"],
            "tabs": [2],
            "active": 2,
            "desktop_project_preferences": {"projects": ["/code/app", "/code/other"]}
        })
        .to_string();
        let sessions = vec![
            (1, "/code/app".into()),
            (2, "/tmp/internal-bot-chat".into()),
            (3, "/private/var/folders/temporary".into()),
        ];
        let restored = Workspace::restore(Some(&saved), &sessions, "/code/app");
        assert_eq!(restored.projects, ["/code/app", "/code/other"]);
        assert_eq!(restored.project, "/code/app");
        assert_eq!(restored.active, None);
        let serialized = serde_json::to_value(restored).unwrap();
        assert_eq!(
            serialized["desktop_project_preferences"]["projects"],
            serde_json::json!(["/code/app", "/code/other"])
        );
    }
    #[test]
    fn restore_prunes_stale_tabs_and_preserves_an_intentionally_empty_workspace() {
        let mut s = Workspace::default();
        s.open(1);
        s.open(8);
        s.tabs.push(1);
        s.sidebar_width = 900.;
        let json = serde_json::to_string(&s).unwrap();
        let s = Workspace::restore(Some(&json), &[(1, "/work/a".into())], "/work/b");
        assert_eq!(s.tabs, vec![1]);
        assert_eq!(s.active, None);
        assert_eq!(s.project, "/work/b");
        assert_eq!(s.sidebar_width, 560.);
        assert_eq!(s.projects, vec!["/work/b"]);
        let s = Workspace::restore(Some("{}"), &[(1, "/work/a".into())], "/work/a");
        assert!(s.tabs.is_empty());
        assert!(
            s.compact_session_cards,
            "older workspaces retain compact cards"
        );
        assert!(
            s.sidebar_activity_visible,
            "the retired activity preference remains readable"
        );
        assert!(
            s.sidebar_subscriptions_visible,
            "existing users keep usage visible"
        );
    }
    #[test]
    fn desktop_snapshot_translation_deserializes_and_retains_unported_metadata() {
        let source = serde_json::json!({
            "projectCwd":"/work", "activeTabId":"tab",
            "tabs":[{"id":"tab", "focusedId":"b", "groupId":"assigned", "editorPanes":[], "layout":{"type":"split", "dir":"down", "children":[{"type":"leaf","id":"a"},{"type":"leaf","id":"b"}], "sizes":[0.4,0.6]}}],
            "projectTerminals":[{"projectPath":"/work", "side":"bottom", "size":240, "open":false, "pane":{"activeFileId":"term", "files":[{"id":"term", "terminal":true, "cwd":"/work/sub"}]}}]
        });
        let mapped = vorkspace_core::desktop_preferences::DesktopPreferences::workspace_snapshot(
            &source,
            |id| match id {
                "a" => Some(1),
                "b" => Some(2),
                _ => None,
            },
        );
        let saved = mapped.to_string();
        let restored = Workspace::restore(
            Some(&saved),
            &[(1, "/work".into()), (2, "/work".into())],
            "/work",
        );
        assert_eq!(restored.tabs, [1]);
        assert_eq!(restored.active, Some(2));
        assert!(restored.layouts[&1].is_split());
        assert_eq!(restored.focused[&1], 2);
        assert_eq!(restored.docks["/work"].terminals, ["/work/sub"]);
        assert!(!restored.docks["/work"].open);
        assert_eq!(restored.desktop_workspace_snapshot.as_ref(), Some(&source));
    }
}
