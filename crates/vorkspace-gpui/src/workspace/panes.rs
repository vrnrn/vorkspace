//! Non-session pane identities. Layout IDs belong to the workspace; they are
//! never sent to the session store or used as synthetic conversations.
use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum AppPane {
    App(AppView),
    Note(String),
    ConnectorItem(Box<vorkspace_core::inbox::Item>),
    Bot(String),
    BotDocument { bot: String, document: String },
    Terminal { id: u64, cwd: String },
    Search,
    Channels,
}

impl AppPane {
    pub fn same_resource(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::ConnectorItem(a), Self::ConnectorItem(b)) => a.key() == b.key(),
            (Self::Terminal { id: a, .. }, Self::Terminal { id: b, .. }) if *a != 0 => a == b,
            _ => self == other,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AppPaneState {
    pub content: AppPane,
    pub project: String,
    pub worktree: String,
}

impl Workspace {
    /// Detached files and app views share one collision-free layout ID space.
    /// Retained descriptors reserve IDs even after a tab closes.
    pub fn fresh_pane_id(&self) -> Option<i64> {
        self.app_panes
            .keys()
            .chain(self.review_owners.keys())
            .chain(self.tabs.iter())
            .chain(self.layouts.keys())
            .copied()
            .chain(self.layouts.values().flat_map(|layout| layout.leaf_ids()))
            .chain(self.title_order.iter().filter_map(TitleTab::session))
            .chain(self.scope_focus.values().filter_map(TitleTab::session))
            .min()
            .unwrap_or(0)
            .min(0)
            .checked_sub(1)
    }

    pub fn app_pane(&mut self, content: AppPane, project: String, worktree: String) -> Option<i64> {
        if let Some(id) = self.app_panes.iter().find_map(|(&id, pane)| {
            (pane.content.same_resource(&content)
                && (!matches!(&content, AppPane::Bot(_))
                    || (vorkspace_core::files::path_key(&pane.project)
                        == vorkspace_core::files::path_key(&project)
                        && vorkspace_core::files::path_key(&pane.worktree)
                            == vorkspace_core::files::path_key(&worktree))))
            .then_some(id)
        }) {
            if let (Some(pane), AppPane::ConnectorItem(current)) =
                (self.app_panes.get_mut(&id), &content)
                && let AppPane::ConnectorItem(cached) = &mut pane.content
                && current.updated_ms() >= cached.updated_ms()
            {
                *cached = current.clone();
            }
            return Some(id);
        }
        let id = self.fresh_pane_id()?;
        self.app_panes.insert(
            id,
            AppPaneState {
                content,
                project,
                worktree,
            },
        );
        Some(id)
    }

    /// Saved standalone destinations now use the same pane tree as sessions.
    /// Keep ordering and scope references while reserving each terminal ID.
    pub(super) fn restore_app_panes(&mut self) {
        self.app_panes
            .retain(|id, _| *id < 0 && !self.review_owners.contains_key(id));
        let mut migrated = HashMap::new();
        let mut legacy = self.title_order.clone();
        let mut scopes = self.scope_focus.iter().collect::<Vec<_>>();
        scopes.sort_by(|a, b| a.0.cmp(b.0));
        legacy.extend(scopes.into_iter().map(|(_, tab)| tab.clone()));
        for tab in legacy {
            if migrated.contains_key(&tab) {
                continue;
            }
            let (content, project, worktree) = match &tab {
                TitleTab::Workspace(_) => continue,
                TitleTab::Bot {
                    id,
                    project,
                    worktree,
                } => (AppPane::Bot(id.clone()), project.clone(), worktree.clone()),
                TitleTab::Terminal { id, cwd } => (
                    AppPane::Terminal {
                        id: *id,
                        cwd: cwd.clone(),
                    },
                    self.project.clone(),
                    cwd.clone(),
                ),
                TitleTab::Search => (AppPane::Search, self.project.clone(), self.project.clone()),
                TitleTab::App(AppView::Inbox) => continue,
                TitleTab::App(view) => (
                    AppPane::App(*view),
                    self.project.clone(),
                    self.project.clone(),
                ),
            };
            if let Some(id) = self.app_pane(content, project, worktree) {
                migrated.insert(tab, TitleTab::Workspace(id));
            }
        }
        for tab in self
            .title_order
            .iter_mut()
            .chain(self.scope_focus.values_mut())
        {
            if let Some(replacement) = migrated.get(tab) {
                *tab = replacement.clone();
            }
        }
    }

    /// Only saved title entries can open standalone app tabs. A scope's last
    /// focus can refer to a closed pane and must not reopen it. Check after
    /// layout normalization so an app already tiled beside a chat stays there.
    pub(super) fn restore_open_app_tabs(&mut self) {
        let ids = self
            .title_order
            .iter()
            .filter_map(TitleTab::session)
            .filter(|id| self.app_panes.contains_key(id))
            .collect::<Vec<_>>();
        for id in ids {
            if self.tab_of(id).is_none() {
                self.tabs.push(id);
            }
        }
    }
}
