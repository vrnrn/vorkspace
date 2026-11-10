mod accounts;
mod app_commands;
mod menus;
mod pane_placement;
mod pane_tree;
mod review_panes;
mod right_panel_host;
mod session_rows;
mod tab_strip;
mod titlebar;
mod workspace_commands;
pub(crate) use crate::bots::host_api::AppStart;
mod app_core;
pub(crate) use app_core::{AppCore, PreparedCore, bot_write_barriers};
mod app_panes;
mod app_views;
mod automation_runs;
pub(crate) mod bots;
mod classic_rail;
mod classic_sidebar;
mod external_editor;
mod file_panes;
#[cfg(test)]
mod fluid_capture_tests;
#[cfg(test)]
mod glide_tests;
#[cfg(test)]
mod hover_cursor_tests;
mod idle_agents;
#[cfg(test)]
mod image_lightbox_tests;
mod layout;
mod live_agents;
mod notifications;
mod orchestration;
#[cfg(test)]
mod perf_tests;
mod picker_preferences;
mod project_delete;
mod project_settings;
mod quick_composer;
mod quick_open;
mod remote_projects;
mod remote_sessions;
mod search;
mod session_filters;
mod session_groups;
mod session_loading;
mod session_preview;
mod settings;
mod shortcuts;
mod sidebar_list;
pub(crate) mod status_menu;
mod terminal_dock;
mod terminal_tabs;
mod title_tabs;
#[cfg(test)]
mod titlebar_tests;
#[cfg(test)]
mod ui_parity_tests;
mod vorkspaces;
mod vox;
mod windows;
mod worktrees;
pub(crate) use windows::{WindowLayouts, additional_window_bounds};

use crate::popover::titlebar_drag_area;
use crate::{
    conversation::{Conversation, SessionChanged},
    input,
    notes::Notes,
    pickers::{ProjectPicked, ProjectPicker},
    popover::{
        Align, Anchor, AnchorSlot, Flyout, Menu, MenuAction, MenuEvent, MenuItem, MenuStyle, Side,
        menu_popover, popover,
    },
    resource_pressure::{ResourceMonitor, StopSession},
    response_actions::Transfer,
    right_panel::RightPanel,
    terminal::{Terminal, TerminalEvent},
    usage_footer::UsageFooter,
    workspace::{
        Connector, DockSide, SidebarNavigation, TitleTab, VisitHistory, Workspace, project_name,
    },
};
use gpui::{
    AnyElement, App, Bounds, BoxShadow, ClipboardItem, Context, CursorStyle, Entity, FocusHandle,
    Focusable, FontWeight, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent,
    PathPromptOptions, Pixels, Point, Role, ScrollHandle, SharedString, StyleRefinement,
    Subscription, Window, actions, div, prelude::*, px, rgb, rgba,
};
use session_groups::local_reminder_label;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};
use vorkspace_core::{provider::Kind, store::Session};
use vorkspace_model::pane_layout::{Edge, LayoutLeaf, PaneNode, Rect, SplitDir};
use vorkspace_runtime::runtime::{Database, locked};
use vorkspace_ui::{
    pane_motion::{PaneSlide, ResizeSettle},
    session_mark::{SessionMark, State as MarkState},
    theme::*,
};

actions!(
    workspace,
    [
        NewCodex,
        NewClaude,
        OpenProject,
        CloseTab,
        CloseAllTabs,
        VisitBack,
        VisitForward,
        NextTab,
        PreviousTab,
        ToggleSidebar,
        FocusSearch,
        SearchFiles,
        FocusNext,
        FocusPrevious,
        CloseOtherTabs,
        PreviousSession,
        NextSession,
        PreviousSessionInTab,
        NextSessionInTab,
        PreviousProject,
        NextProject,
        SwitchModel,
        ToggleRightPanel,
        SplitRight,
        SplitDown,
        FocusPaneLeft,
        FocusPaneRight,
        FocusPaneUp,
        FocusPaneDown,
        ToggleTerminal,
        NewTerminal,
        OpenSettings,
        GoToFile,
        OpenCommandPalette,
        ShowInbox,
        ShowGitHub,
        ShowGitLab,
        ShowJira,
        ShowNotes,
        ShowAutomations,
        OpenAppearance,
        MinimizeWindow,
        ZoomWindow,
        ToggleFullScreen
    ]
);

#[derive(Clone, Copy)]
enum ChromeKind {
    Sidebar,
    ProjectRail,
    Tabs,
}
struct Chrome {
    owner: gpui::WeakEntity<Shell>,
    kind: ChromeKind,
    renders: usize,
}
impl Render for Chrome {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let kind = self.kind;
        self.owner
            .update(cx, |shell, cx| match kind {
                ChromeKind::Sidebar => shell.sidebar(cx),
                ChromeKind::Tabs => shell.tabs(window, cx),
                ChromeKind::ProjectRail => shell.classic_project_rail(window, cx),
            })
            .unwrap_or_else(|_| div().into_any())
    }
}

/// `Tab: Activate 1–8` with the slot, or `Tab: Activate Last` with -1.
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = workspace, no_json)]
pub(crate) struct ActivateTab(pub isize);

/// The workspace's keys, after TitleBar/App's `tabCommand` and the settings
/// keybinding table: tabs, history, sessions, projects and the sidebar.
pub(crate) fn bind_keys(cx: &mut App) {
    session_groups::bind(cx);
    let modifier = "cmd";
    // Scoped to the shell, so the focused composer, menus and pickers outrank
    // these: GPUI ranks an unscoped binding above every context.
    let context = Some("Workspace");
    let window_context = Some("Workspace || VorkspaceModal");
    cx.bind_keys([
        KeyBinding::new(
            &format!("{modifier}-="),
            crate::image_lightbox::ZoomIn,
            window_context,
        ),
        KeyBinding::new(
            &format!("{modifier}-+"),
            crate::image_lightbox::ZoomIn,
            window_context,
        ),
        KeyBinding::new(
            &format!("{modifier}--"),
            crate::image_lightbox::ZoomOut,
            window_context,
        ),
        KeyBinding::new(
            &format!("{modifier}-0"),
            crate::image_lightbox::ResetZoom,
            window_context,
        ),
        KeyBinding::new(&format!("{modifier}-m"), MinimizeWindow, context),
        KeyBinding::new(&format!("{modifier}-n"), NewCodex, context),
        KeyBinding::new(&format!("{modifier}-t"), NewCodex, context),
        KeyBinding::new(&format!("{modifier}-shift-n"), NewClaude, context),
        KeyBinding::new(&format!("{modifier}-o"), OpenProject, context),
        KeyBinding::new(&format!("{modifier}-p"), GoToFile, context),
        KeyBinding::new(&format!("{modifier}-shift-p"), OpenCommandPalette, context),
        KeyBinding::new(&format!("{modifier}-,"), OpenSettings, context),
        KeyBinding::new(&format!("{modifier}-w"), CloseTab, context),
        KeyBinding::new(&format!("{modifier}-shift-w"), CloseAllTabs, context),
        KeyBinding::new(&format!("{modifier}-["), VisitBack, context),
        KeyBinding::new(&format!("{modifier}-]"), VisitForward, context),
        KeyBinding::new("escape", vorkspace_ui::actions::DismissMenu, window_context),
        KeyBinding::new(&format!("{modifier}-b"), ToggleSidebar, context),
        KeyBinding::new(&format!("{modifier}-k"), FocusSearch, context),
        KeyBinding::new(&format!("{modifier}-shift-f"), SearchFiles, context),
        KeyBinding::new(&format!("{modifier}-alt-t"), CloseOtherTabs, context),
        KeyBinding::new(&format!("{modifier}-shift-]"), NextTab, context),
        KeyBinding::new(&format!("{modifier}-}}"), NextTab, context),
        KeyBinding::new(&format!("{modifier}-shift-["), PreviousTab, context),
        KeyBinding::new(&format!("{modifier}-{{"), PreviousTab, context),
        KeyBinding::new(&format!("{modifier}-1"), ActivateTab(0), context),
        KeyBinding::new(&format!("{modifier}-2"), ActivateTab(1), context),
        KeyBinding::new(&format!("{modifier}-3"), ActivateTab(2), context),
        KeyBinding::new(&format!("{modifier}-4"), ActivateTab(3), context),
        KeyBinding::new(&format!("{modifier}-5"), ActivateTab(4), context),
        KeyBinding::new(&format!("{modifier}-6"), ActivateTab(5), context),
        KeyBinding::new(&format!("{modifier}-7"), ActivateTab(6), context),
        KeyBinding::new(&format!("{modifier}-8"), ActivateTab(7), context),
        KeyBinding::new(&format!("{modifier}-9"), ActivateTab(-1), context),
        KeyBinding::new(&format!("{modifier}-shift-up"), PreviousSession, context),
        KeyBinding::new(&format!("{modifier}-shift-down"), NextSession, context),
        KeyBinding::new(&format!("{modifier}-up"), PreviousSessionInTab, context),
        KeyBinding::new(&format!("{modifier}-down"), NextSessionInTab, context),
        KeyBinding::new(&format!("{modifier}-shift-left"), PreviousProject, context),
        KeyBinding::new(&format!("{modifier}-shift-right"), NextProject, context),
        KeyBinding::new(&format!("{modifier}-."), SwitchModel, context),
        KeyBinding::new(&format!("{modifier}-shift-b"), ToggleRightPanel, context),
        KeyBinding::new(&format!("{modifier}-d"), SplitRight, context),
        KeyBinding::new(&format!("{modifier}-j"), ToggleTerminal, context),
        KeyBinding::new(&format!("{modifier}-`"), NewTerminal, context),
        KeyBinding::new(&format!("{modifier}-shift-d"), SplitDown, context),
        KeyBinding::new(&format!("{modifier}-alt-left"), FocusPaneLeft, context),
        KeyBinding::new(&format!("{modifier}-alt-right"), FocusPaneRight, context),
        KeyBinding::new(&format!("{modifier}-alt-up"), FocusPaneUp, context),
        KeyBinding::new(&format!("{modifier}-alt-down"), FocusPaneDown, context),
        KeyBinding::new("ctrl-tab", NextTab, context),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, context),
        KeyBinding::new("tab", FocusNext, window_context),
        KeyBinding::new("shift-tab", FocusPrevious, window_context),
    ]);
}

/// tabKeys.ts's `adjacentItemId`: the next or previous item, wrapping; from
/// outside the list, the first or last.
fn adjacent<T: PartialEq + Clone>(items: &[T], current: Option<&T>, step: isize) -> Option<T> {
    if items.is_empty() {
        return None;
    }
    let len = items.len() as isize;
    match current.and_then(|c| items.iter().position(|item| item == c)) {
        Some(ix) => Some(items[(ix as isize + step.signum()).rem_euclid(len) as usize].clone()),
        None if step < 0 => items.last().cloned(),
        None => items.first().cloned(),
    }
}

#[derive(Clone, PartialEq)]
pub(crate) enum MenuTarget {
    Tab(i64),
    TitleTab(TitleTab),
    Tabs(Vec<TitleTab>),
    Session(i64),
    /// The sidebar project header's menu.
    Header,
    SessionFilter,
    SidebarOptions,
    /// A project's actions; `rail` adds the project rail's grouping and pinning.
    Project {
        path: String,
        rail: bool,
    },
    /// The rail's vorkspace switcher.
    Vorkspace,
    /// ProjectTerminalDock's Move Terminal menu.
    TerminalDock,
    /// A dock terminal tab's context menu.
    TerminalTab(usize),
}
pub(crate) struct ShellMenu {
    pub(crate) target: MenuTarget,
    anchor: Anchor,
    menu: Entity<Menu>,
    _events: Vec<Subscription>,
}
// The tab strip's padding; the trailing side also has the 2px gap before the
// strip's drag area.
const STRIP_LEAD: f32 = 6.;
const STRIP_TRAIL: f32 = 6.;
const STRIP_PAD: f32 = STRIP_LEAD + STRIP_TRAIL;
/// The selected title tab shrinks with the others only down to this.
const SELECTED_TAB_MIN_WIDTH: f32 = 112.;
/// Narrower tabs drop their close button.
const TAB_CLOSE_MIN_WIDTH: f32 = 72.;
/// The new-tab button and its leading margin.
const STRIP_NEW_TAB: f32 = TAB_PLATE_HEIGHT + 2.;
// TitleBar.tsx's labels, with their shortcuts.
const TOGGLE_SIDEBAR_LABEL: &str = "Toggle Rail (⌘B)";
const TOGGLE_EXPLORER_LABEL: &str = "Toggle Sidebar (⇧⌘B)";
const MOD: &str = "⌘";
const REVEAL_LABEL: &str = "Reveal in Finder";

struct TabDrag {
    id: TitleTab,
    from: usize,
    to: usize,
    origin: Pixels,
    offset: Pixels,
    active: bool,
    bounds: Vec<Bounds<Pixels>>,
    /// Pulled far enough below the strip to drop on a pane.
    armed: bool,
}

/// What a pane drop carries: a split pane by its header, a title tab
/// pulled down out of the strip, or a sidebar session.
#[derive(Clone, Copy, Debug, PartialEq)]
enum DragSource {
    Pane(i64),
    Review(i64),
    Tab(i64),
    Session(i64),
}
impl DragSource {
    fn session(self) -> i64 {
        match self {
            DragSource::Pane(id)
            | DragSource::Review(id)
            | DragSource::Tab(id)
            | DragSource::Session(id) => id,
        }
    }
}
/// PaneTree.tsx's pane drag and the external drops from TitleBar/Sidebar.
struct PaneDrag {
    source: DragSource,
    start: Point<Pixels>,
    position: Point<Pixels>,
    active: bool,
    over: Option<(i64, Edge)>,
    /// A pane pulled onto the title strip becomes a tab at this index.
    insert: Option<usize>,
}
struct SashDrag {
    tab: i64,
    path: Vec<usize>,
    index: usize,
    dir: SplitDir,
    group: Rect,
}
/// Something the user resizes by dragging its edge.
#[derive(Clone, PartialEq)]
enum ResizeTarget {
    Sidebar,
    ProjectRail,
    Dock(String),
    /// A pane sash, with its group's length in pixels.
    Sash {
        tab: i64,
        path: Vec<usize>,
        index: usize,
        span: f32,
    },
}
/// A pane split into the tree pushes in from its edge while its neighbours
/// follow; a closed pane's neighbours grow back over its room.
struct PaneMotion {
    tab: i64,
    from: HashMap<i64, Rect>,
    entering: HashMap<i64, Edge>,
    /// Closed panes riding their edge out: the old leaf, its edge and the
    /// collapsed rect it ends at.
    leaving: Vec<(LayoutLeaf, Edge, Rect)>,
    /// The split header grows as a lone pane gains a neighbour, and shrinks
    /// as the split closes back to one pane.
    header: Option<bool>,
    slide: PaneSlide,
}
struct TabCopy {
    headline: String,
    /// Every pane in layout order; the tab shows one line at a time.
    panes: Vec<TabPane>,
    /// The pane in `panes` that has focus.
    focus: usize,
}
/// One line of a tab's headline: a chat, or the file beside it.
struct TabPane {
    /// The pane's leaf, and whether this is its file side.
    key: (i64, bool),
    title: String,
    mark: PaneMark,
}
enum PaneMark {
    /// The provider, with the session id when its mark is lit.
    Session(String, Option<i64>),
    /// A file's name, or `None` for changes and commits.
    File(Option<String>),
    /// A registered workspace app pane's icon.
    App(&'static str),
    /// The same appearance snapshot used by the Bot's sidebar avatar.
    Bot(crate::avatar::BotLookConfig),
}
/// A split tab's headline rolling to the newly focused pane: the next pane
/// rises from below, the previous one drops from above.
struct PaneTick {
    shown: (i64, bool),
    from: Option<(i64, bool)>,
    /// 1 when the new line rises, -1 when it drops.
    dir: f32,
    start: vorkspace_ui::clock::Instant,
}
const PANE_TICK_DURATION: f32 = 0.24;
/// useAnimatedReorder: `--motion-reorder-duration` with `--motion-ease-out`.
const REORDER_DURATION: f32 = 0.16;
/// A tab easing from one horizontal offset to another.
#[derive(Clone, Copy)]
struct TabShift {
    from: f32,
    to: f32,
    start: vorkspace_ui::clock::Instant,
}
impl TabShift {
    fn at(&self, now: vorkspace_ui::clock::Instant) -> (f32, bool) {
        let t = now.saturating_duration_since(self.start).as_secs_f32() / REORDER_DURATION;
        if t >= 1. {
            return (self.to, true);
        }
        let eased = vorkspace_ui::reveal::ease_move(t);
        (self.from + (self.to - self.from) * eased, false)
    }
}
/// Below this a chat column stops being comfortable to read.
const MIN_CHAT_WIDTH: f32 = 420.;
const MAX_SESSION_STACK: usize = 3;
const PANE_LIMIT_MESSAGE: &str = "Not enough room to tile here. Widen the window or use another tab; columns can contain up to three panes.";
/// A sideways tab drag sags; only a deliberate pull below the strip
/// targets a pane (TitleBar's PANE_DROP_PULL).
const PANE_DROP_PULL: f32 = 32.;

struct SessionPreview {
    id: i64,
    data: Option<vorkspace_core::store::SessionPreviewData>,
    anchor: AnchorSlot,
    surface: AnchorSlot,
}

pub(crate) struct Shell {
    bots: Option<Entity<crate::bots::Bots>>,
    vox: Option<Entity<crate::vox::Vox>>,
    quick_open: Option<quick_open::Open>,
    image_lightbox: Option<(Entity<crate::image_lightbox::ImageLightbox>, Subscription)>,
    orchestrations: HashMap<i64, orchestration::Run>,
    /// Each lead's last orchestration, whose agents hang under it in the sidebar.
    orchestration_summaries: HashMap<i64, vorkspace_core::orchestration::Summary>,
    /// Sessions an /operator thread started, by the session that started them.
    started_by: HashMap<i64, i64>,
    /// The sidebar rails under orchestrators and /operator threads.
    usage_footer: Entity<UsageFooter>,
    /// Resource pressure indicator and its running-work dialog.
    resources: Entity<ResourceMonitor>,
    sidebar_frame: Entity<Chrome>,
    project_rail_frame: Entity<Chrome>,
    project_rail_drag: Option<(Pixels, f32)>,
    sidebar_search: Entity<input::Input>,
    sidebar_slide: Option<PaneSlide>,
    sidebar_fraction: f32,
    sidebar_content_width: f32,
    /// useTrafficLightsVisible: fullscreen hides the native buttons, so the
    /// title bar takes their room back.
    fullscreen: bool,
    tabs_frame: Entity<Chrome>,
    store: Database,
    remote: Arc<vorkspace_core::remote::Connections>,
    remote_project_picker: Option<remote_projects::Open>,
    remote_marks: HashMap<i64, (bool, bool)>,
    sessions: HashMap<i64, Entity<Conversation>>,
    /// Lightweight history index; only opened or previewed chats own GPUI entities.
    session_summaries: HashMap<i64, Session>,
    draft_sessions: HashSet<i64>,
    visible_chats: HashSet<i64>,
    history_recency: HashMap<i64, u64>,
    history_clock: u64,
    session_marks: HashMap<i64, Entity<SessionMark>>,
    tab_marks: HashMap<i64, Entity<SessionMark>>,
    busy_sessions: HashSet<i64>,
    notification_state: notifications::State,
    quick: crate::composer::quick_composer::Controller,
    unseen_finished: HashSet<i64>,
    live_agents: Entity<live_agents::LiveAgents>,
    archiving: HashSet<i64>,
    groups: HashMap<String, Arc<session_groups::FolderIndex>>,
    renaming: Option<session_groups::Renaming>,
    link_dialog: Option<session_groups::LinkDialog>,
    deleting: HashSet<i64>,
    session_removal_epoch: u64,
    group_writes: Option<gpui::Task<()>>,
    selected_sessions: HashSet<i64>,
    selection_anchor: Option<i64>,
    context_selection: bool,
    sidebar_target: Option<session_groups::RenameTarget>,
    session_row_bounds: HashMap<i64, AnchorSlot>,
    filter_anchor: AnchorSlot,
    _session_clock: gpui::Task<()>,
    notes: Option<Entity<Notes>>,
    codebase_tabs: HashMap<i64, Entity<crate::codebase::Page>>,
    note_tabs: HashMap<i64, Entity<Notes>>,
    item_tabs: HashMap<i64, Entity<crate::inbox::Inbox>>,
    bot_document_tabs: HashMap<i64, Entity<crate::bots::BotDocumentPane>>,
    bot_pane_views: HashMap<i64, Entity<app_panes::BotPaneView>>,
    todos_page: Option<Entity<crate::todos::TodosPage>>,
    /// The project sidebar's board; it follows the current project.
    project_todos: Option<Entity<crate::todos::TodoBoardView>>,
    /// Minion sessions' context and capability, shared with Bots.
    minion_registry: crate::conversation::MinionRegistry,
    team_histories: crate::conversation::TeamHistories,
    bot_conversations: crate::bots::BotConversations,
    inbox: Option<Entity<crate::inbox::Inbox>>,
    connector_pages: HashMap<crate::workspace::Connector, Entity<crate::inbox::Inbox>>,
    automations: Option<Entity<crate::automations::Automations>>,
    active_app_view: Option<app_views::AppView>,
    active_terminal_tab: Option<u64>,
    /// Title-tab shells have exclusive ownership, separate from dock shells.
    terminal_tabs: HashMap<u64, Entity<Terminal>>,
    app_view_events: Vec<Subscription>,
    automation_state: automation_runs::State,
    settings: Option<Entity<crate::settings::SettingsView>>,
    settings_events: Option<Subscription>,
    app_search: Option<search::Open>,
    parked_search: Option<search::Open>,
    preparing_panes: bool,
    saved_search: Option<(String, vorkspace_core::search::Scope)>,
    order: Vec<i64>,
    pub(crate) workspace: Workspace,
    external_editors: Option<Vec<vorkspace_core::external_editor::ExternalEditor>>,
    external_editor_scan: u64,
    deleting_projects: HashSet<String>,
    active_worktree_removals: usize,
    focus: FocusHandle,
    project_rail_toggle_focus: FocusHandle,
    sidebar_focus: FocusHandle,
    query: String,
    sidebar_hover: Option<&'static str>,
    tab_hover: Option<i64>,
    /// Tab animations: opening and closing title tabs, with each tab's last label.
    title_tab_motion: vorkspace_ui::tab_motion::TabMotion<TitleTab, ()>,
    /// Tab animations: the selected tab before the last switch, the one now
    /// selected and when it changed, so one eases down as the other widens.
    title_tab_selection: (
        Option<TitleTab>,
        Option<TitleTab>,
        vorkspace_ui::clock::Instant,
    ),
    session_hover: Option<i64>,
    session_preview: Option<SessionPreview>,
    preview_close: Option<gpui::Task<()>>,
    preview_open: Option<gpui::Task<()>>,
    sidebar_last_scroll: Option<vorkspace_ui::clock::Instant>,
    window_observed: bool,
    shortcuts: Option<shortcuts::Shortcuts>,
    window_active: bool,
    window_glass: Option<bool>,
    window_blur: Option<u32>,
    quit_prompted: bool,
    pub(crate) menu: Option<ShellMenu>,
    background_dialog: Option<project_settings::OpenBackground>,
    /// Project settings: one project's appearance, background, Codebase
    /// Memory and notifications, in the same dialog frame as a Bot's.
    pub(super) vorkspace_dialog: Option<vorkspaces::VorkspaceDialog>,
    /// What every window shares; `None` only before the window joins it.
    app_core: Option<Entity<app_core::AppCore>>,
    preparing_sessions: Arc<std::sync::atomic::AtomicUsize>,
    vorkspace_baseline: Arc<Vec<crate::workspace::VorkspaceDef>>,
    window_layout: u64,
    window_shared: Option<Arc<windows::WindowShared>>,
    window_closing: bool,
    native_window: Option<gpui::WindowHandle<Self>>,
    native_title: String,
    rail_sessions_expanded: bool,
    rail_list: gpui::ListState,
    rail_list_ids: std::rc::Rc<Vec<i64>>,
    /// The rail's title search field is showing.
    rail_search_open: bool,
    vorkspace_anchor: AnchorSlot,
    backgrounds: HashMap<i64, Entity<crate::project_background::ProjectBackground>>,
    /// Each project's working grid in the rail, by project path.
    project_anchor: AnchorSlot,
    sidebar_options_anchor: AnchorSlot,
    history: VisitHistory,
    /// User navigation invalidates pending requests that would otherwise
    /// focus a new session in a tab or project the user has already left.
    navigation_revision: u64,
    pending_project_blanks: HashSet<String>,
    pending_project_creations: HashMap<String, usize>,
    new_tab_task: Option<gpui::Task<()>>,
    worktree_focus: HashMap<String, crate::git_tools::worktree_switcher::WorktreeFocus>,
    session_worktrees: HashMap<i64, Option<String>>,
    worktree_returns: HashMap<(String, String), i64>,
    tab_drag: Option<TabDrag>,
    /// Tabs sliding between slots: neighbours making room for a drag and,
    /// after a drop, every tab gliding from where it was drawn into place.
    tab_shifts: HashMap<TitleTab, TabShift>,
    pane_ticks: HashMap<i64, PaneTick>,
    skip_tab_click: bool,
    error: Option<String>,
    tab_scroll: ScrollHandle,
    tab_switcher_anchor: AnchorSlot,
    tab_scroll_task: Option<gpui::Task<()>>,
    resize: Option<(Pixels, f32)>,
    pub(crate) git: Entity<crate::git_status::ProjectGit>,
    pub(crate) right_panel: Option<Entity<RightPanel>>,
    reviews: HashMap<i64, Entity<crate::git_tools::review::ReviewPane>>,
    review_cache: crate::git_tools::review::ReviewCache,
    /// A review sliding in or out: its session and pane.
    review_slide: Option<(i64, PaneSlide, Entity<crate::git_tools::review::ReviewPane>)>,
    /// A session review card's Review, opened beside its pane next frame.
    pending_review: Option<(i64, crate::git_tools::review::ReviewTab)>,
    pending_composer_focus: Option<i64>,
    review_focused: bool,
    review_drag: bool,
    split_bounds: HashMap<i64, AnchorSlot>,
    pane_bounds: AnchorSlot,
    pane_viewport: AnchorSlot,
    pane_scroll: ScrollHandle,
    pane_scroll_focus: Option<(i64, i64, u32, u32)>,
    pane_drag: Option<PaneDrag>,
    pane_sash: Option<SashDrag>,
    pane_motion: Option<PaneMotion>,
    skip_session_click: bool,
    /// Live terminals of each project's dock, in the dock's tab order.
    terminals: HashMap<String, Vec<Entity<Terminal>>>,
    /// A project's dock sliding open (toward 1) or shut (toward 0).
    dock_slide: Option<(String, PaneSlide)>,
    dock_resize: Option<(f32, f32)>,
    /// A resize bouncing off the limit it was dragged into.
    resize_settle: Option<(ResizeTarget, ResizeSettle)>,
    /// The drag is pressing past its limit, so it has already bounced.
    resize_pinned: bool,
    dock_hover: Option<usize>,
    /// Tab animations for the dock's terminals, with each tab's last title.
    dock_tab_motion: vorkspace_ui::tab_motion::TabMotion<gpui::EntityId, SharedString>,
    dock_drag: Option<terminal_dock::DockTabDrag>,
    /// The right panel's drawn width this frame, 0 while it is hidden.
    /// Classic: the corner tab opening out and the one folding away.
    dock_shifts: HashMap<gpui::EntityId, TabShift>,
    dock_skip_click: bool,
    dock_tab_scroll: ScrollHandle,
    move_terminal_anchor: AnchorSlot,
    /// The sidebar collapsed itself to make room; it returns with the room.
    viewport_width: f32,
    viewport_height: f32,
    observations: Vec<Subscription>,
    pub(crate) render_passes: usize,
}
impl Shell {
    fn open_image(
        &mut self,
        open: &crate::image_lightbox::Open,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((old, _)) = &self.image_lightbox {
            old.update(cx, |view, cx| view.reject_replacement(cx));
            cx.stop_propagation();
            return;
        }
        let owner = open.draft_session.and_then(|id| {
            self.sessions.get(&id).cloned().or_else(|| {
                self.bots
                    .as_ref()
                    .and_then(|bots| bots.read(cx).conversation_for_session(id, cx))
            })
        });
        let owner = owner
            .as_ref()
            .map(crate::conversation::composer_features::annotation_owner);
        let view =
            cx.new(|cx| crate::image_lightbox::ImageLightbox::new(open.clone(), owner, window, cx));
        let closed = cx.subscribe(&view, |this, _, _: &crate::image_lightbox::Closed, cx| {
            this.image_lightbox = None;
            cx.notify();
        });
        self.image_lightbox = Some((view, closed));
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn new(
        store: Database,
        summaries: Vec<Session>,
        cwd: String,
        saved: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        vorkspace_ui::clock::use_test_clock(cx);
        let paths = summaries
            .iter()
            .map(|s| (s.id, s.cwd.clone()))
            .collect::<Vec<_>>();
        let workspace = Workspace::restore(saved.as_deref(), &paths, &cwd);
        let mut projects = workspace.projects;
        if !workspace.project.is_empty() && !projects.contains(&workspace.project) {
            projects.push(workspace.project);
        }
        let metadata = locked(&store)
            .map(|db| db.session_sidebar_metadata(&projects))
            .unwrap_or_default();
        Self::new_with_metadata(store, summaries, cwd, saved, metadata, None, cx)
    }
    pub(crate) fn new_with_metadata(
        store: Database,
        summaries: Vec<Session>,
        cwd: String,
        saved: Option<String>,
        sidebar_metadata: vorkspace_core::store::SessionSidebarMetadata,
        window_layout: Option<u64>,
        cx: &mut Context<Self>,
    ) -> Self {
        vorkspace_ui::fluid::init(cx);
        let app_core = app_core::AppCore::get(&store, cx);
        crate::accounts::install(store.clone(), cx);
        crate::codebase::install(store.clone(), cx);
        if let Some(owner) = crate::codebase::get(cx) {
            cx.observe(&owner, |this, _, cx| {
                this.invalidate_chrome(cx);
                cx.notify();
            })
            .detach();
        }
        let remote = app_core.read(cx).remote.clone();
        let metadata: Vec<_> = summaries.iter().map(|s| (s.id, s.cwd.clone())).collect();
        let mut workspace = Workspace::restore(saved.as_deref(), &metadata, &cwd);
        let layouts = &app_core.read(cx).layouts;
        let window_shared = layouts.shared.clone();
        if window_layout.is_some() && window_layout == layouts.focused {
            workspace
                .restored_new_projects
                .extend(layouts.imported_projects.iter().cloned());
        }
        if let Some(shared) = &window_shared {
            let discovered = workspace.restored_new_projects.clone();
            shared.apply(&mut workspace);
            for project in discovered {
                if workspace.is_project_available(&project) {
                    workspace.add_project(&project);
                }
            }
        }
        vorkspace_core::cli::initialize(&workspace.preferences.binary_paths);
        vorkspace_core::worktree_setup::set_commands(&workspace.preferences.worktree_setup);
        let owner = cx.weak_entity();
        let sidebar_frame = cx.new(|_| Chrome {
            owner: owner.clone(),
            kind: ChromeKind::Sidebar,
            renders: 0,
        });
        let tabs_frame = cx.new(|_| Chrome {
            owner: owner.clone(),
            kind: ChromeKind::Tabs,
            renders: 0,
        });
        let project_rail_frame = cx.new(|_| Chrome {
            owner,
            kind: ChromeKind::ProjectRail,
            renders: 0,
        });
        let sidebar_search = cx.new(input::Input::search);
        sidebar_search.update(cx, |input, _| input.set_font_size(12.));
        let sidebar_search_events =
            cx.subscribe(&sidebar_search, |this, search, _: &input::Changed, cx| {
                this.query = search.read(cx).content.to_lowercase();
                this.notify_sidebar(cx);
                cx.notify();
            });
        let active_kind = summaries
            .iter()
            .find(|s| Some(s.id) == workspace.active)
            .map(|s| Kind::from_id(&s.provider))
            .unwrap_or(Kind::Codex);
        let session_clock = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(30))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        this.poll_automations(cx);
                        this.update_chat_visibility(cx);
                        if this.workspace.sidebar_visible {
                            this.notify_sidebar(cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let git_project = workspace.project.clone();
        let checkouts = app_core.read(cx).checkouts.clone();
        let git = cx.new(|cx| crate::git_status::ProjectGit::shared(git_project, checkouts, cx));
        let resources = cx.new(ResourceMonitor::new);
        let resource_observation = cx.observe(&resources, |_, _, cx| cx.notify());
        let resource_stops = cx.subscribe(&resources, |this, _, request: &StopSession, cx| {
            this.stop_resource_turn(request, cx);
        });
        let worktree_focus = workspace.worktree_focus.clone();
        let session_worktrees = sidebar_metadata.worktrees;
        let github_activity = app_core.read(cx).github_activity.clone();
        let notification_state = notifications::State::new(&workspace, github_activity, cx);
        let owner = cx.weak_entity();
        let live_agents = cx.new(|cx| live_agents::LiveAgents::new(owner, cx));
        let mut this = Self {
            bots: None,
            vox: None,
            quick_open: None,
            image_lightbox: None,
            orchestrations: HashMap::new(),
            orchestration_summaries: sidebar_metadata.orchestrations,
            started_by: sidebar_metadata.started_by,
            usage_footer: cx.new(|cx| UsageFooter::new(active_kind, cx)),
            resources,
            sidebar_frame,
            project_rail_frame,
            project_rail_drag: None,
            sidebar_search,
            sidebar_slide: None,
            sidebar_fraction: if workspace.sidebar_visible { 1. } else { 0. },
            sidebar_content_width: workspace.right_panel_width,
            fullscreen: false,
            tabs_frame,
            store,
            remote,
            remote_project_picker: None,
            remote_marks: HashMap::new(),
            sessions: HashMap::new(),
            session_summaries: HashMap::new(),
            draft_sessions: sidebar_metadata.drafts,
            visible_chats: HashSet::new(),
            history_recency: HashMap::new(),
            history_clock: 0,
            session_marks: HashMap::new(),
            tab_marks: HashMap::new(),
            busy_sessions: HashSet::new(),
            notification_state,
            quick: Default::default(),
            unseen_finished: HashSet::new(),
            live_agents,
            archiving: HashSet::new(),
            groups: sidebar_metadata
                .groups
                .into_iter()
                .map(|(project, groups)| {
                    (project, Arc::new(session_groups::FolderIndex::new(groups)))
                })
                .collect(),
            renaming: None,
            link_dialog: None,
            deleting: HashSet::new(),
            session_removal_epoch: 0,
            group_writes: None,
            selected_sessions: HashSet::new(),
            selection_anchor: None,
            context_selection: false,
            sidebar_target: None,
            session_row_bounds: HashMap::new(),
            filter_anchor: AnchorSlot::default(),
            _session_clock: session_clock,
            notes: None,
            codebase_tabs: HashMap::new(),
            note_tabs: HashMap::new(),
            item_tabs: HashMap::new(),
            bot_document_tabs: HashMap::new(),
            bot_pane_views: HashMap::new(),
            todos_page: None,
            project_todos: None,
            minion_registry: Default::default(),
            team_histories: Default::default(),
            bot_conversations: Default::default(),
            inbox: None,
            connector_pages: HashMap::new(),
            automations: None,
            active_app_view: None,
            active_terminal_tab: None,
            terminal_tabs: HashMap::new(),
            app_view_events: Vec::new(),
            automation_state: Default::default(),
            settings: None,
            settings_events: None,
            app_search: None,
            parked_search: None,
            preparing_panes: false,
            saved_search: None,
            order: Vec::new(),
            workspace,
            external_editors: None,
            external_editor_scan: 0,
            deleting_projects: HashSet::new(),
            active_worktree_removals: 0,
            focus: cx.focus_handle(),
            project_rail_toggle_focus: cx.focus_handle().tab_stop(true),
            sidebar_focus: cx.focus_handle(),
            query: String::new(),
            sidebar_hover: None,
            tab_hover: None,
            title_tab_motion: Default::default(),
            title_tab_selection: (None, None, vorkspace_ui::clock::Instant::now()),
            session_hover: None,
            session_preview: None,
            preview_close: None,
            preview_open: None,
            sidebar_last_scroll: None,
            window_observed: false,
            shortcuts: None,
            window_active: false,
            window_glass: None,
            window_blur: None,
            quit_prompted: false,
            menu: None,
            background_dialog: None,
            vorkspace_dialog: None,
            app_core: None,
            preparing_sessions: Default::default(),
            vorkspace_baseline: Arc::default(),
            window_layout: window_layout
                .unwrap_or_else(|| app_core.update(cx, |core, _| core.allocate_window())),
            window_shared,
            window_closing: false,
            native_window: None,
            native_title: String::new(),
            rail_sessions_expanded: false,
            rail_list: gpui::ListState::new(0, gpui::ListAlignment::Top, px(96.)),
            rail_list_ids: Default::default(),
            rail_search_open: false,
            vorkspace_anchor: AnchorSlot::default(),
            backgrounds: HashMap::new(),

            project_anchor: AnchorSlot::default(),
            sidebar_options_anchor: AnchorSlot::default(),
            history: VisitHistory::default(),
            navigation_revision: 0,
            pending_project_blanks: HashSet::new(),
            pending_project_creations: HashMap::new(),
            new_tab_task: None,
            worktree_focus,
            session_worktrees,
            worktree_returns: HashMap::new(),
            tab_drag: None,
            tab_shifts: HashMap::new(),
            pane_ticks: HashMap::new(),
            skip_tab_click: false,
            error: None,
            tab_scroll: ScrollHandle::new(),
            tab_switcher_anchor: AnchorSlot::default(),
            tab_scroll_task: None,
            resize: None,
            git,
            right_panel: None,
            reviews: HashMap::new(),
            review_cache: Default::default(),
            review_slide: None,
            pending_review: None,
            pending_composer_focus: None,
            review_focused: false,
            review_drag: false,
            split_bounds: HashMap::new(),
            pane_bounds: AnchorSlot::default(),
            pane_viewport: AnchorSlot::default(),
            pane_scroll: ScrollHandle::new(),
            pane_scroll_focus: None,
            pane_drag: None,
            pane_sash: None,
            pane_motion: None,
            skip_session_click: false,
            terminals: HashMap::new(),
            dock_slide: None,
            dock_resize: None,
            resize_settle: None,
            resize_pinned: false,
            dock_hover: None,
            dock_tab_motion: Default::default(),
            dock_drag: None,
            dock_shifts: HashMap::new(),
            dock_skip_click: false,
            dock_tab_scroll: ScrollHandle::new(),
            move_terminal_anchor: AnchorSlot::default(),
            viewport_width: 1200.,
            viewport_height: 800.,
            observations: vec![resource_observation, resource_stops, sidebar_search_events],
            render_passes: 0,
        };
        this.scan_external_editors(cx);
        this.adopt_app_core(app_core, cx);
        this.observe_accounts(cx);
        this.observe_picker_preferences(cx);
        let bot_owner: std::rc::Rc<dyn crate::bots::host_api::BotsHost> =
            std::rc::Rc::new(cx.weak_entity());
        let bot_active =
            this.workspace
                .scope_focus
                .get(&this.tab_scope())
                .and_then(|tab| match tab {
                    TitleTab::Bot { id, .. } => Some(id.clone()),
                    _ => None,
                });
        let roster = this.app_core.as_ref().unwrap().read(cx).bot_roster.clone();
        let bots = cx.new(|cx| {
            crate::bots::Bots::new(
                roster,
                bot_owner,
                this.workspace.preferences.clone(),
                bot_active,
                Some(this.workspace.active_vorkspace.clone()),
                this.notification_state.region.clone(),
                cx,
            )
        });
        // Views opened before Bots existed share its registry too.
        this.minion_registry = bots.read(cx).minion_registry.clone();
        this.team_histories = bots.read(cx).team_histories.clone();
        this.bot_conversations = bots.read(cx).conversation_views.clone();
        for view in this.sessions.values() {
            let registry = this.minion_registry.clone();
            view.update(cx, |view, cx| {
                view.minions = Some(registry);
                view.team_histories = Some(this.team_histories.clone());
                view.refresh_team_history(cx);
            });
        }
        this.observations.push(
            cx.subscribe(&bots, |this, _, _: &crate::bots::Changed, cx| {
                if this.prune_deleted_bot_tabs(cx) {
                    this.persist(cx);
                }
                // Restored legacy panes share the durable classification even
                // when their original project-session view is still cached.
                for view in this.sessions.values() {
                    if view.read(cx).user_read_only() {
                        view.update(cx, |view, cx| {
                            view.refresh_team_history(cx);
                        });
                    }
                }
                this.sync_bot_search(cx);
                this.refresh_bot_document_panes(cx);
                this.update_usage_provider(cx);
                this.invalidate_chrome(cx);
                cx.notify();
            }),
        );
        this.observations.push(cx.subscribe(
            &bots,
            |this, _, opened: &crate::bots::TabOpened, cx| {
                this.bot_opened(opened.0.clone(), cx);
            },
        ));
        this.observations.push(cx.subscribe(
            &bots,
            |this, _, request: &crate::bots::OpenWorkspacePane, cx| {
                let request = request.clone();
                this.navigation_revision += 1;
                let revision = this.navigation_revision;
                cx.spawn(async move |this, cx| {
                    let _ = this.update_in(cx, |this, window, cx| {
                        if this.navigation_revision == revision {
                            this.open_bot_workspace_pane(request, window, cx);
                        }
                    });
                })
                .detach();
            },
        ));
        let mut bot_error = bots.read(cx).error.clone();
        this.observations
            .push(cx.observe(&bots, move |_, bots, cx| {
                let error = &bots.read(cx).error;
                if *error != bot_error {
                    bot_error = error.clone();
                    cx.notify();
                }
            }));
        this.bots = Some(bots);
        this.prune_deleted_bot_tabs(cx);
        let voice_owner: crate::vox::Host = std::rc::Rc::new(cx.weak_entity());
        crate::vox::set_workspace_toggle(
            |cx| {
                let Some(target) = workspace_target(cx) else {
                    return;
                };
                let _ = target.update(cx, |root, window, cx| {
                    if let Ok(shell) = root.downcast::<Shell>() {
                        shell.update(cx, |shell, cx| shell.toggle_vox(window, cx));
                    }
                });
            },
            cx,
        );
        this.vox = Some(cx.new(|_| {
            crate::vox::Vox::new(
                voice_owner,
                this.store.clone(),
                &this.workspace.preferences,
                this.workspace.project.clone(),
            )
        }));
        if let Some(voice) = &this.vox {
            crate::vox::register(voice, &this.workspace.preferences, cx);
            this.observations.push(cx.observe(voice, |this, _, cx| {
                this.sync_settings(cx);
                this.project_rail_frame.update(cx, |_, cx| cx.notify());
                cx.notify();
            }));
        }
        this.bind_notices(cx);
        this.init_app_bridge(cx);
        for summary in summaries {
            if this
                .bots
                .as_ref()
                .is_some_and(|m| m.read(cx).is_bot_session(summary.id))
            {
                continue;
            }
            this.order.push(summary.id);
            this.session_summaries.insert(summary.id, summary);
        }
        if let Some(id) = this
            .workspace
            .active
            .and_then(|id| this.workspace.session_for_pane(id))
        {
            this.ensure_session(id, cx);
            this.record_worktree_session(id, cx);
        }
        if let Some(id) = this.workspace.active {
            this.history.record(id);
        }
        this.reveal_active_tab(cx);
        this.update_chat_visibility(cx);
        this.update_usage_provider(cx);
        this.apply_preferences(cx);
        this.initialize_automations(cx);
        cx.spawn(async move |this, cx| {
            let _ = this.update_in(cx, |this, window, cx| {
                if this.navigation_revision == 0 {
                    this.sync_vorkspace_bot_visibility(cx);
                    // Sidebar worktree selection resets at launch. An active
                    // app pane restores from its layout.
                    let saved_app = this.workspace.active.filter(|id| {
                        this.workspace.app_panes.get(id).is_some_and(|pane| {
                            pane.content.project_scoped()
                                && this.app_pane_available(&pane.content, cx)
                        })
                    });
                    if let Some(id) = saved_app {
                        this.open(id, window, cx);
                    } else if !this.restore_scope_tab(true, window, cx) {
                        this.ensure_project_session(window, cx);
                    }
                }
                this.persist(cx);
            });
        })
        .detach();
        this
    }
    fn update_chat_visibility(&mut self, cx: &mut Context<Self>) {
        let shown: HashSet<i64> = self
            .workspace
            .active_tab()
            .filter(|_| self.settings.is_none())
            .map(|tab| {
                self.workspace
                    .layout(tab)
                    .leaf_ids()
                    .into_iter()
                    .filter(|id| {
                        self.workspace
                            .app_panes
                            .get(id)
                            .is_none_or(|pane| self.app_pane_available(&pane.content, cx))
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Hidden tabs keep their prepared image and any in-flight load. Only
        // closing a tab/pane should discard its background.
        if !self.backgrounds.is_empty() {
            let open: HashSet<_> = self.workspace.open_sessions().into_iter().collect();
            self.backgrounds.retain(|id, _| open.contains(id));
        }
        let mut visible_chats = HashSet::with_capacity(shown.len());
        for &id in &shown {
            if self.ensure_session(id, cx).is_some() {
                visible_chats.insert(id);
            }
        }
        let visible_entities = visible_chats
            .iter()
            .map(|id| self.sessions[id].entity_id())
            .collect();
        let bot_chats = self.update_app_pane_visibility(&shown, &visible_entities, cx);
        for id in self.visible_chats.difference(&visible_chats) {
            if let Some(view) = self.sessions.get(id)
                && !bot_chats.contains(&view.entity_id())
            {
                view.read(cx)
                    .transcript
                    .clone()
                    .update(cx, |t, cx| t.visible(false, cx));
            }
        }
        for &id in visible_chats.difference(&self.visible_chats) {
            self.history_clock += 1;
            self.history_recency.insert(id, self.history_clock);
            let view = &self.sessions[&id];
            let bot_owns_visibility = bot_chats.contains(&view.entity_id());
            view.update(cx, |v, cx| {
                if !bot_owns_visibility {
                    v.transcript.update(cx, |t, cx| t.visible(true, cx));
                }
                v.load(cx);
            });
        }
        self.visible_chats = visible_chats;
        self.trim_session_histories(cx);
    }
    #[cfg(test)]
    fn add(&mut self, summary: Session, cx: &mut Context<Self>) {
        self.workspace.restore_project(&summary.cwd);
        let id = summary.id;
        self.add_loaded(summary, cx);
        self.publish_session_summary(id, cx);
    }

    fn install_prepared(
        &mut self,
        summary: Session,
        install: &crate::session_install::Install,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Conversation>> {
        if self.sessions.contains_key(&summary.id) {
            return None;
        }
        if let Err(error) = self.verify_prepared_checkout(summary.id, None, cx) {
            self.error = Some(error);
            cx.notify();
            return None;
        }
        let project = summary.cwd.clone();
        let view = self.add_loaded_with_install(summary, Some(install), cx)?;
        self.workspace.restore_project(&project);
        Some(view)
    }

    fn session_install_failed(&mut self, cx: &mut Context<Self>) {
        self.error
            .get_or_insert_with(|| "The new session could not be opened in this window.".into());
        cx.notify();
    }

    fn verify_prepared_checkout(
        &self,
        id: i64,
        requested: Option<&str>,
        cx: &App,
    ) -> Result<(), String> {
        let saved = locked(&self.store)?
            .agent_preferences(id)?
            .unwrap_or_default();
        let current = saved.worktree_cwd.as_deref();
        if saved.worktree_removed
            || requested.zip(current).is_some_and(|(requested, current)| {
                vorkspace_core::files::path_key(requested)
                    != vorkspace_core::files::path_key(current)
            })
        {
            return Err("The session's worktree changed before it could open.".into());
        }
        if let Some(path) = requested.or(current) {
            if self.project_is_deleting(path, cx)
                || vorkspace_core::worktree_management::is_being_removed(path)
            {
                return Err("The session's worktree is being removed.".into());
            }
            if requested.is_some()
                && !path.starts_with("remote://")
                && !std::path::Path::new(path).is_dir()
            {
                return Err("The session's worktree is no longer available.".into());
            }
        }
        Ok(())
    }

    fn publish_prepared(
        &mut self,
        view: &Entity<Conversation>,
        install: Option<&mut crate::session_install::Install>,
        cx: &mut Context<Self>,
    ) {
        let view = view.read(cx);
        let id = view.summary.id;
        self.session_summaries.insert(id, view.summary.clone());
        self.session_worktrees
            .insert(id, view.settings.worktree_cwd.clone());
        if view.has_saved_draft() {
            self.draft_sessions.insert(id);
        } else {
            self.draft_sessions.remove(&id);
        }
        if let Some(install) = install {
            install.expose();
        }
        self.publish_session_summary(id, cx);
    }

    /// Loading history must not turn every session directory into a rail project.
    fn add_loaded(
        &mut self,
        summary: Session,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Conversation>> {
        self.add_loaded_with_install(summary, None, cx)
    }

    fn add_loaded_with_install(
        &mut self,
        summary: Session,
        install: Option<&crate::session_install::Install>,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Conversation>> {
        let id = summary.id;
        if let Some(view) = self.sessions.get(&id) {
            return Some(view.clone());
        }
        let worktree = self.session_worktrees.get(&id).cloned().flatten();
        if !self.allow_project_entry(&summary.cwd, cx)
            || worktree
                .as_deref()
                .is_some_and(|path| !self.allow_project_entry(path, cx))
        {
            return None;
        }
        if !self.claim_session(id, install, cx) {
            return None;
        }
        self.session_summaries.insert(id, summary.clone());
        self.session_marks
            .insert(id, cx.new(|_| SessionMark::new(summary.provider.clone())));
        self.tab_marks
            .insert(id, cx.new(|_| SessionMark::tab(summary.provider.clone())));
        let appearance = self.appearance(&summary.cwd);
        let existing = self
            .bot_conversations
            .borrow()
            .get(&id)
            .and_then(|view| view.upgrade());
        let bridge = self.bridge_environment(id, cx);
        let new_view = existing.is_none();
        let view = existing
            .unwrap_or_else(|| cx.new(|cx| Conversation::new(summary, self.store.clone(), cx)));
        view.update(cx, |view, cx| {
            // Events may arrive before the first asynchronous agent load. Seed
            // the indexed working directory so those events cannot erase it.
            view.restore_worktree(worktree, None, cx);
            view.draft_hint = self.draft_sessions.contains(&id);
            if let Some(environment) = bridge {
                view.set_app_environment(environment);
            }
            view.minions = Some(self.minion_registry.clone());
            view.team_histories = Some(self.team_histories.clone());
            view.refresh_team_history(cx);
            view.set_project_appearance(appearance, cx);
            view.transcript.update(cx, |transcript, cx| {
                transcript.apply_preferences(&self.workspace.preferences, cx);
                if new_view {
                    transcript.visible(false, cx);
                }
            });
        });
        self.observations.push(
            cx.subscribe(&view, move |this, _, event: &SessionChanged, cx| {
                this.session_changed(id, *event, cx);
            }),
        );
        self.observations.push(cx.subscribe(
            &view,
            move |this, view, _: &crate::conversation::SessionActivityChanged, cx| {
                let label = view.read(cx).activity.clone();
                this.live_agents
                    .update(cx, |activity, cx| activity.set_activity(id, label, cx));
            },
        ));
        self.observations
            .push(cx.subscribe(&view, |this, _, transfer: &Transfer, cx| {
                this.transfer_response(transfer.clone(), cx)
            }));
        self.observations.push(cx.subscribe(
            &view,
            move |this, _, event: &crate::conversation::OpenSessionChanges, cx| {
                let tab = event.0.clone();
                // The card's pane takes focus, so its review opens beside it.
                this.pending_review = Some((id, tab));
                cx.notify();
            },
        ));
        self.observations
            .push(cx.observe(&view, move |this, _, cx| {
                if this
                    .session_preview
                    .as_ref()
                    .is_some_and(|preview| preview.id == id)
                {
                    this.sidebar_frame.update(cx, |_, cx| cx.notify());
                    this.invalidate_chrome(cx);
                    cx.notify();
                }
            }));
        self.observe_orchestration(&view, id, cx);
        self.sessions.insert(id, view.clone());
        if !self.order.contains(&id) {
            self.order.push(id);
        }
        Some(view)
    }
    /// SessionPane's reviewUndoLocked: another running session shares the checkout.
    fn sync_review_locks(&mut self, cx: &mut Context<Self>) {
        let running: Vec<(i64, String)> = self
            .sessions
            .iter()
            .filter(|(_, view)| view.read(cx).busy)
            .map(|(&id, view)| (id, view.read(cx).review.read(cx).session().1.to_owned()))
            .collect();
        for (&id, view) in &self.sessions {
            let review = view.read(cx).review.clone();
            let cwd = review.read(cx).session().1.to_owned();
            let locked = running
                .iter()
                .any(|(other, path)| *other != id && *path == cwd);
            review.update(cx, |review, cx| review.set_undo_locked(locked, cx));
        }
    }
    fn refresh_session_mark(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(id) = self.workspace.session_for_pane(id) else {
            return;
        };
        let Some(view) = self.sessions.get(&id) else {
            return;
        };
        let session = view.read(cx);
        let busy = session.responding();
        let focused = !self.app_view_open()
            && !self.bot_active(cx)
            && self.settings.is_none()
            && self.app_search.is_none()
            && self.workspace.active == Some(id);
        if busy {
            self.busy_sessions.insert(id);
            self.unseen_finished.remove(&id);
        } else if session.busy {
            // Explicit cancellation is already idle in the UI. The worker is
            // only finishing its save, so it must not announce a completed turn.
            self.busy_sessions.remove(&id);
            self.unseen_finished.remove(&id);
        } else if self.busy_sessions.remove(&id) && !focused && !session.summary.archived {
            self.unseen_finished.insert(id);
        }
        if focused || session.summary.archived {
            self.unseen_finished.remove(&id);
        }
        let state = if busy {
            MarkState::Working
        } else if self.unseen_finished.contains(&id) {
            MarkState::Done
        } else {
            MarkState::Idle
        };
        // A blocked turn is still busy, but the tab asks for attention first.
        let attention = session.needs_approval();
        let tab_state = if attention {
            MarkState::Attention
        } else {
            state
        };
        let provider = session.summary.provider.clone();
        let cwd = session.summary.cwd.clone();
        // A working session's grid takes its project's colour.
        let color = self.appearance(&cwd).color(&cwd);
        self.live_agents.update(cx, |activity, _| {
            activity.record(id, busy, self.unseen_finished.contains(&id));
        });
        self.session_marks[&id].update(cx, |mark, cx| {
            mark.set_color(color, cx);
            mark.set(&provider, state, cx)
        });
        self.tab_marks[&id].update(cx, |mark, cx| {
            mark.set_color(color, cx);
            mark.set(&provider, tab_state, cx)
        });
    }
    fn toggle_archive(&mut self, id: i64, cx: &mut Context<Self>) {
        if !self.session_available_here(id, cx) {
            self.error =
                Some("Open the conversation in its window to change its archive status.".into());
            cx.notify();
            return;
        }
        if !self.archiving.insert(id) {
            return;
        }
        self.begin_session_metadata(&[id], cx);
        self.close_preview(cx);
        let archived = !self.session_metadata(id, cx).summary.archived;
        let store = self.store.clone();
        let work = cx
            .background_executor()
            .spawn(async move { locked(&store)?.set_archived(id, archived) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.archiving.remove(&id);
                this.finish_session_metadata(&[id], cx);
                this.complete_linked_cleanup(id, archived && result.is_ok(), cx);
                match result {
                    Ok(()) => {
                        if let Some(summary) = this.session_summaries.get_mut(&id) {
                            summary.archived = archived;
                        }
                        if let Some(view) = this.sessions.get(&id) {
                            view.update(cx, |session, cx| {
                                session.summary.archived = archived;
                                if archived {
                                    session.stop_if_busy(cx);
                                }
                            });
                        }
                        this.refresh_session_mark(id, cx);
                        this.publish_session_summary(id, cx);
                        if archived && this.workspace.tab_of(id).is_some() {
                            this.close_pane(id, window, cx);
                        } else {
                            this.invalidate_chrome(cx);
                            cx.notify();
                        }
                    }
                    Err(error) => {
                        this.error = Some(format!("Could not update archive: {error}"));
                        this.invalidate_chrome(cx);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
        self.invalidate_chrome(cx);
    }
    fn transfer_response(&mut self, transfer: Transfer, cx: &mut Context<Self>) {
        use vorkspace_core::{
            agent::{Handoff, Settings},
            response::{self, TransferKind},
        };
        let navigation_revision = self.navigation_revision;
        let store = self.store.clone();
        let target_kind = Kind::from_id(&transfer.target.provider);
        let account = crate::accounts::get(cx)
            .map(|center| center.read(cx).selected(target_kind, &transfer.cwd))
            .unwrap_or_default();
        let mut install = self.session_install(cx);
        let work = cx.background_executor().spawn(async move {
            let mut settings = Settings {
                account,
                revision: response::now_ms() * 1_000_000,
                model: transfer.target.model.clone(),
                effort: transfer.target.effort.clone(),
                model_settings: transfer.target.settings.clone(),
                access: transfer.settings.access,
                worktree_cwd: transfer.settings.worktree_cwd.clone(),
                ..Settings::default()
            };
            let prompt = if transfer.kind == TransferKind::SecondOpinion {
                Some(response::second_opinion(&transfer.from, &transfer.turn))
            } else {
                settings.handoff = Some(Handoff {
                    from: transfer.from.clone(),
                    brief: response::handoff(&transfer.through),
                });
                None
            };
            let db = locked(&store)?;
            let mut summary = db.create(&transfer.target.provider, &transfer.cwd)?;
            install.track(summary.id);
            summary.title = if transfer.kind == TransferKind::SecondOpinion {
                "Second opinion".into()
            } else {
                transfer.title
            };
            db.title(summary.id, &summary.title)?;
            db.configure_agent(summary.id, &summary.provider, &settings)?;
            Ok::<_, String>((summary, settings, prompt, install))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok((summary, settings, prompt, mut install)) => {
                    let id = summary.id;
                    let Some(view) = this.install_prepared(summary, &install, cx) else {
                        this.session_install_failed(cx);
                        return;
                    };
                    view.update(cx, |view, cx| view.install_transfer(settings, prompt, cx));
                    this.publish_prepared(&view, Some(&mut install), cx);
                    this.order.retain(|s| *s != id);
                    this.order.insert(0, id);
                    if this.navigation_revision == navigation_revision {
                        this.activate_session(id, window, cx);
                    }
                }
                Err(error) => {
                    this.error = Some(error);
                    cx.notify();
                }
            });
        })
        .detach();
    }
    fn notify_sidebar(&self, cx: &mut Context<Self>) {
        self.sync_live_agents(cx);
        self.sidebar_frame.update(cx, |_, cx| cx.notify());
        self.project_rail_frame.update(cx, |_, cx| cx.notify());
    }
    fn invalidate_chrome(&self, cx: &mut Context<Self>) {
        self.notify_sidebar(cx);
        self.tabs_frame.update(cx, |_, cx| cx.notify());
    }
    fn reveal_active_tab(&self, cx: &App) {
        if let Some(ix) = self
            .visible_tabs(cx)
            .iter()
            .position(|tab| Some(tab) == self.active_title_tab(cx).as_ref())
        {
            self.tab_scroll.scroll_to_item(ix);
        }
    }
    /// Sidebar.tsx's `gitRoot`: the active session's worktree or folder.
    fn sync_git(&mut self, cx: &mut Context<Self>) {
        let session = self
            .workspace
            .active
            .and_then(|id| self.workspace.session_for_pane(id))
            .and_then(|id| self.sessions.get(&id))
            .map(|view| {
                let view = view.read(cx);
                (
                    view.settings
                        .worktree_cwd
                        .clone()
                        .unwrap_or_else(|| view.summary.cwd.clone()),
                    Kind::from_id(&view.summary.provider),
                )
            });
        let (project, provider) = match session {
            Some((cwd, kind)) => (cwd, Some(kind)),
            None => (self.working_copy().to_owned(), None),
        };
        self.git.update(cx, |git, cx| git.set_project(project, cx));
        if let (Some(panel), Some(kind)) = (&self.right_panel, provider) {
            let changes = panel.read(cx).changes.clone();
            changes.update(cx, |c, _| c.text_provider = kind);
        }
        self.sync_right_panel(cx);
    }
    /// The active tab's review pane, created from its saved state.
    fn unsaved_files(&self, cx: &App) -> bool {
        self.reviews
            .values()
            .any(|pane| pane.read(cx).unsaved_files(cx))
    }

    fn prepare_notes_close(&self, cx: &mut Context<Self>) -> bool {
        self.notes
            .iter()
            .chain(self.note_tabs.values())
            .all(|note| note.update(cx, |note, cx| note.prepare_close(cx)))
    }

    fn scale_interface(&mut self, delta: i32, window: &mut Window, cx: &mut Context<Self>) {
        let scale = if delta == 0 {
            100
        } else {
            self.workspace
                .preferences
                .ui_scale
                .saturating_add_signed(delta)
                .clamp(
                    vorkspace_ui::preferences::Preferences::MIN_UI_SCALE_PERCENT,
                    vorkspace_ui::preferences::Preferences::MAX_UI_SCALE_PERCENT,
                )
        };
        if self.workspace.preferences.ui_scale == scale {
            return;
        }
        self.workspace.preferences.ui_scale = scale;
        window.set_ui_scale(scale as f32 / 100., cx);
        self.apply_preferences(cx);
        self.sync_settings(cx);
        self.invalidate_chrome(cx);
        self.persist(cx);
        cx.notify();
    }
    fn request_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.unsaved_window_edits(cx) {
            crate::quit(cx);
            return;
        }
        if self.quit_prompted {
            return;
        }
        self.quit_prompted = true;
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            "Quit with unsaved edits?",
            Some("Save your edits before quitting to keep your changes."),
            &["Quit Without Saving", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await;
            let _ = this.update(cx, |this, cx| {
                this.quit_prompted = false;
                if matches!(answer, Ok(0)) {
                    crate::quit(cx);
                } else {
                    crate::background_helper::quit_cancelled(cx);
                }
            });
        })
        .detach();
    }
    fn request_restart(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.unsaved_window_edits(cx) {
            crate::restart(cx);
            return;
        }
        if self.quit_prompted {
            return;
        }
        self.quit_prompted = true;
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            "Restart with unsaved edits?",
            Some("Save your edits before restarting to keep your changes."),
            &["Restart Without Saving", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await;
            let _ = this.update(cx, |this, cx| {
                this.quit_prompted = false;
                if matches!(answer, Ok(0)) {
                    crate::restart(cx);
                }
            });
        })
        .detach();
    }
    /// requestAddToChat(text, "plain"): appended to the tab's draft as its
    /// own paragraph.
    fn add_to_chat(&mut self, id: i64, text: &str, cx: &mut Context<Self>) -> Result<(), String> {
        if self.deleting.contains(&id) {
            return Err("The originating chat is being deleted.".into());
        }
        let view = self
            .ensure_session(id, cx)
            .ok_or("The originating chat is unavailable.")?;
        view.update(cx, |chat, cx| chat.append_review_comment(text, cx))?;
        self.chat_hidden_off(id, cx);
        self.pending_composer_focus = Some(id);
        cx.notify();
        Ok(())
    }
    fn chat_hidden_off(&mut self, id: i64, cx: &mut Context<Self>) {
        if let Some(review) = self.reviews.get(&id) {
            review.update(cx, |r, cx| {
                if r.state.chat_hidden {
                    r.state.chat_hidden = false;
                    cx.emit(crate::git_tools::review::ReviewEvent::Changed);
                }
            });
        }
    }
    fn resume_git(&mut self, cx: &mut Context<Self>) {
        self.git.update(cx, |git, cx| git.resume(cx));
        if let Some(panel) = &self.right_panel {
            let panel = panel.read(cx);
            let (explorer, changes, history) = (
                panel.explorer.clone(),
                panel.changes.clone(),
                panel.history.clone(),
            );
            explorer.update(cx, |e, cx| e.resume(cx));
            changes.update(cx, |c, cx| c.resume(cx));
            history.update(cx, |h, cx| h.resume(cx));
        }
    }
    fn persist(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.workspace.active {
            self.refresh_session_mark(id, cx);
        }
        self.workspace.worktree_focus = self.worktree_focus.clone();
        self.workspace.normalize_title_order();
        self.sync_git(cx);
        self.sync_inbox_scope(cx);
        self.update_chat_visibility(cx);
        self.update_usage_provider(cx);
        self.invalidate_chrome(cx);
        self.share_vorkspaces(cx);
        if let Err(error) = self.save_window_state(None, false, cx) {
            self.error = Some(format!("Could not save workspace: {error}"));
            cx.notify();
        }
    }
    fn update_usage_provider(&self, cx: &mut Context<Self>) {
        let view = self
            .bots
            .as_ref()
            .filter(|_| self.bot_active(cx))
            .and_then(|m| m.read(cx).chat())
            .or_else(|| {
                self.workspace
                    .active
                    .and_then(|id| self.workspace.session_for_pane(id))
                    .and_then(|id| self.sessions.get(&id).cloned())
            });
        if let Some(view) = view {
            let view = view.read(cx);
            let kind = Kind::from_id(&view.summary.provider);
            let profile = view.settings.account.clone();
            let model = view.account_model().map(str::to_owned);
            let project = view.summary.cwd.clone();
            let session = Some(view.summary.id);
            self.usage_footer.update(cx, |footer, cx| {
                footer.context(kind, profile, model.as_deref(), project, session, cx)
            });
        }
    }
    pub(crate) fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(scope) = self.management_modal_focus(cx) {
            if !scope.contains_focused(window, cx) {
                window.focus(&scope, cx);
            }
            return;
        }
        self.keep_visible_idle_agents(cx);
        if let Some(bots) = self.bots.clone()
            && bots.update(cx, |bots, cx| bots.focus_creation(window, cx))
        {
            return;
        }
        if let Some(search) = &self.app_search {
            window.focus(&search.view.focus_handle(cx), cx);
            return;
        }
        if let Some(view) = self
            .workspace
            .active
            .and_then(|id| self.codebase_tabs.get(&id))
        {
            window.focus(&view.focus_handle(cx), cx);
            return;
        }
        if self.focus_app_view(window, cx) {
            return;
        }
        if let Some(crate::workspace::AppPane::Bot(id)) = self
            .workspace
            .active
            .and_then(|id| self.workspace.app_panes.get(&id))
            .map(|pane| &pane.content)
        {
            let id = id.clone();
            if let Some(manager) = self.bots.clone() {
                manager.update(cx, |m, cx| m.focus_bot_pane(&id, window, cx));
            }
            return;
        }
        if let Some(view) = self
            .workspace
            .active
            .and_then(|id| self.bot_document_tabs.get(&id))
        {
            window.focus(&view.focus_handle(cx), cx);
            return;
        }
        if let Some(bots) = self.bots.clone().filter(|_| self.bot_active(cx)) {
            bots.update(cx, |m, cx| m.focus_active(window, cx));
            return;
        }
        if let Some(review) = self
            .active_review(cx)
            .filter(|review| review.read(cx).state.chat_hidden)
        {
            review.update(cx, |r, cx| r.focus_active(window, cx));
            return;
        }
        if let Some(view) = self.workspace.active.and_then(|id| self.sessions.get(&id)) {
            let focus = view.read(cx).input.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
        } else {
            window.focus(&self.focus, cx);
        }
    }
    pub(crate) fn menu_focus(&self) -> gpui::FocusHandle {
        self.focus.clone()
    }
    pub(crate) fn open(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.open_with_reminder(id, None, window, cx);
    }

    fn open_with_reminder(
        &mut self,
        id: i64,
        occurrence: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(owner) = self.workspace.session_for_pane(id) {
            if self.focus_session_window(owner, cx) {
                return;
            }
            if self.deleting.contains(&owner) {
                return;
            }
            let Some(view) = self.ensure_session(owner, cx) else {
                return;
            };
            let project = view.read(cx).summary.cwd.clone();
            if !self.route_project(&project, cx) {
                return;
            }
        }
        self.prepare_tab_views(id, window, cx);
        if self.workspace.app_panes.contains_key(&id) {
            self.open_app_pane(id, window, cx);
            return;
        }
        if self.workspace.review_owners.contains_key(&id) {
            self.open_file_pane(id, window, cx);
            return;
        }
        if !self.session_summaries.contains_key(&id) || self.deleting.contains(&id) {
            return;
        }
        self.navigation_revision += 1;
        self.replace_blank_pane(id, cx);
        self.activate_session_with_reminder(id, occurrence, window, cx);
    }
    /// Opening a conversation that has no tab takes over a blank pane in the
    /// active tab instead of adding a tab, as in the desktop app.
    fn replace_blank_pane(&mut self, id: i64, cx: &mut Context<Self>) {
        if self.workspace.tab_of(id).is_some()
            || !self.session_metadata(id, cx).summary.has_messages
        {
            return;
        }
        let Some(tab) = self.workspace.active_tab() else {
            return;
        };
        let cwd = &self.session_metadata(id, cx).summary.cwd;
        let working_copy = vorkspace_core::files::path_key(self.session_working_copy(id, cx));
        let blank = |pane: i64| {
            if self.workspace.review_owners.contains_key(&pane)
                || !self.session_summaries.contains_key(&pane)
            {
                return false;
            }
            let s = self.session_metadata(pane, cx);
            &s.summary.cwd == cwd
                && vorkspace_core::files::path_key(self.session_working_copy(pane, cx))
                    == working_copy
                && !s.summary.has_messages
                && !s.busy
                && !s.has_saved_draft()
                && self
                    .sessions
                    .get(&pane)
                    .is_none_or(|v| v.read(cx).input.read(cx).content.is_empty())
        };
        let focused = self.workspace.tab_focus(tab);
        let Some(pane) = std::iter::once(focused)
            .chain(self.workspace.layout(tab).leaf_ids())
            .find(|pane| blank(*pane))
        else {
            return;
        };
        self.workspace.swap_session(pane, id);
        if let Some(next) = self.workspace.tab_of(id) {
            self.replace_title_tab_key(tab, next);
        }
    }
    fn activate_session(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.activate_session_with_reminder(id, None, window, cx);
    }

    fn activate_session_with_reminder(
        &mut self,
        id: i64,
        occurrence: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace.review_owners.contains_key(&id) {
            self.open_file_pane(id, window, cx);
            return;
        }
        let Some(view) = self.ensure_session(id, cx) else {
            return;
        };
        let project = view.read(cx).summary.cwd.clone();
        if !self.route_project(&project, cx) {
            return;
        }
        self.leave_app_views(cx);
        self.leave_app_search(cx);
        self.close_preview(cx);
        if let Some(due_at) = occurrence {
            self.clear_notification_reminder(id, due_at, cx);
        } else {
            self.dismiss_due_reminder(id, cx);
        }
        self.history.record(id);
        self.workspace.open(id);
        self.workspace.project = project;
        self.workspace
            .project_focus
            .insert(self.workspace.project.clone(), id);
        self.workspace.collapsed.remove(&self.workspace.project);
        if self.visible_chats.contains(&id) {
            view.update(cx, |v, cx| v.load(cx));
        }
        self.record_worktree_session(id, cx);
        self.remember_title_tab(TitleTab::Workspace(self.workspace.tab_of(id).unwrap()));
        self.reveal_active_tab(cx);
        self.review_focused = self
            .workspace
            .reviews
            .get(&id)
            .is_some_and(|review| review.chat_hidden);
        self.focus_active(window, cx);
        self.persist(cx);
        cx.notify();
    }
    pub(crate) fn close(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tab_can_close(id, cx) {
            return;
        }
        self.close_menu(cx);
        self.close_tabs(&[id], None, window, cx);
    }
    /// Apply a close operation to one snapshot. Intermediate neighbours must
    /// never change the project used for the final selection or blank tab.
    fn close_tabs(
        &mut self,
        ids: &[i64],
        preferred: Option<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tabs = ids
            .iter()
            .filter_map(|id| self.workspace.tab_of(*id))
            .map(TitleTab::Workspace)
            .collect::<Vec<_>>();
        let preferred = preferred
            .and_then(|id| self.workspace.tab_of(id))
            .map(TitleTab::Workspace);
        self.close_title_tabs(&tabs, preferred, window, cx);
    }
    fn visit(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.history.step(step) {
            self.open(id, window, cx);
        }
    }
    fn cycle(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        let tabs = self.visible_tabs(cx);
        if tabs.is_empty() {
            return;
        }
        let active = self.active_title_tab(cx);
        let current = tabs.iter().position(|tab| Some(tab) == active.as_ref());
        let next = current.map_or(if step < 0 { tabs.len() - 1 } else { 0 }, |index| {
            (index as isize + step).rem_euclid(tabs.len() as isize) as usize
        });
        self.open_title_tab(&tabs[next], window, cx);
    }
    fn focus_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(scope) = self.management_modal_focus(cx) {
            if !scope.contains_focused(window, cx) {
                window.focus(&scope, cx);
            }
            return;
        }
        if let Some(settings) = &self.settings {
            window.focus(&settings.focus_handle(cx), cx);
            return;
        }
        window.focus(&self.focus, cx);
        if let Some(view) = self
            .workspace
            .active
            .and_then(|id| self.codebase_tabs.get(&id))
        {
            window.focus(&view.focus_handle(cx), cx);
            return;
        }
        if self.focus_app_view(window, cx) {
            return;
        }
        if self.review_focused
            && let Some(review) = self.active_review(cx)
        {
            review.update(cx, |r, cx| r.focus_active(window, cx));
        } else {
            self.focus_active(window, cx);
        }
    }
    fn toggle_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_preview(cx);
        if self.workspace.sidebar_visible && self.sidebar_focus.contains_focused(window, cx) {
            self.focus_workspace(window, cx);
        }
        self.set_sidebar_visible(!self.workspace.sidebar_visible, cx);
        self.persist(cx);
        cx.notify();
    }
    fn set_sidebar_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if visible == self.workspace.sidebar_visible {
            return;
        }
        let to = if visible { 1. } else { 0. };
        self.sidebar_slide = Some(match self.sidebar_slide.take() {
            Some(slide) => slide.retarget(to, vorkspace_ui::clock::Instant::now()),
            None => PaneSlide::new(1. - to, to, self.workspace.right_panel_width),
        });
        self.resize = None;
        self.workspace.sidebar_visible = visible;
        self.sync_right_panel(cx);
    }
    fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_app_search(window, cx);
    }

    pub(crate) fn select_project(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.enter_project(path, false, window, cx)
    }
    /// A deliberate click on a project means "show me this project": it
    /// always leaves a visible Bot and lands on a session, even when the
    /// Bot belongs to that project or was the project's last focused tab.
    pub(crate) fn open_project_from_user(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.enter_project(path, true, window, cx)
    }
    fn enter_project(
        &mut self,
        path: String,
        explicit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.allow_project_entry(&path, cx) {
            return;
        }
        if !path.is_empty()
            && self.workspace.is_project_available(&path)
            && !self.workspace.projects.iter().any(|p| {
                vorkspace_core::files::path_key(p) == vorkspace_core::files::path_key(&path)
            })
        {
            self.workspace.add_project(&path);
        }
        if !self.route_project(&path, cx) {
            return;
        }
        // Programmatic reselection of the visible Bot's project keeps its
        // live detail editor; a user's click on the project leaves the Bot.
        if !explicit
            && path == self.workspace.project
            && self.settings.is_none()
            && let Some(bot) = self.workspace_bot_context()
            && self
                .bots
                .as_ref()
                .is_some_and(|manager| manager.read(cx).active.as_deref() == Some(bot.as_str()))
            && let Some(tab) = self.active_title_tab(cx)
            && self.workspace.scope_focus.get(&self.tab_scope()) == Some(&tab)
            && self.visible_tabs(cx).contains(&tab)
        {
            return;
        }
        self.selected_sessions.clear();
        self.selection_anchor = None;
        self.sidebar_target = None;
        self.navigation_revision += 1;
        self.leave_app_views(cx);
        self.leave_app_search(cx);
        self.close_preview(cx);
        if let Some(bots) = self.bots.clone() {
            bots.update(cx, |m, cx| m.close(cx));
        }
        self.workspace.project = path.clone();
        self.workspace.active = None;
        if !path.is_empty() {
            self.workspace.collapsed.remove(&path);
        }
        // Restore the last scoped tab before falling back to a session; an
        // explicit project click never lands back on the Bot it just left.
        if self.restore_scope_tab(!explicit, window, cx) {
            return;
        }
        self.workspace.active = self
            .workspace
            .project_focus
            .get(&path)
            .copied()
            .filter(|id| self.workspace.tab_of(*id).is_some() && self.pane_in_scope(*id, cx))
            // The project's last focus may be the Bot being left.
            .filter(|id| !explicit || !self.is_bot_pane(*id))
            .or_else(|| {
                self.visible_tabs(cx)
                    .iter()
                    .rev()
                    .find_map(|tab| tab.session())
                    .map(|tab| self.workspace.tab_focus(tab))
            });
        if let Some(id) = self.workspace.active {
            self.open(id, window, cx);
        } else {
            self.ensure_project_session(window, cx);
        }
        self.focus_active(window, cx);
        self.persist(cx);
        cx.notify();
    }
    fn ensure_project_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.project.is_empty()
            || self.workspace.active.is_some()
            || self.bot_active(cx)
            || self.active_terminal_tab.is_some()
        {
            return;
        }
        if let Some(id) = self.order.iter().copied().find(|id| {
            let s = self.session_metadata(*id, cx);
            self.session_available_here(*id, cx)
                && s.summary.cwd == self.workspace.project
                && vorkspace_core::files::path_key(self.session_working_copy(*id, cx))
                    == vorkspace_core::files::path_key(self.working_copy())
                && !s.summary.archived
                && !s.summary.has_messages
                && !s.busy
                && !s.has_saved_draft()
                && self
                    .sessions
                    .get(id)
                    .is_none_or(|v| v.read(cx).input.read(cx).content.is_empty())
                && self.workspace.tab_of(*id).is_none()
                && self
                    .workspace
                    .reviews
                    .get(id)
                    .is_none_or(|r| r.tabs.is_empty())
        }) {
            self.activate_session(id, window, cx);
        } else {
            self.create_session(self.default_kind(), true, cx);
        }
    }
    fn open_project(&mut self, cx: &mut Context<Self>) {
        let target = self.project_picker_target();
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open project".into()),
        });
        cx.spawn(async move |this, cx| match picker.await {
            Ok(Ok(Some(paths))) => {
                if let Some(path) = paths.first() {
                    let path = path.to_string_lossy().into_owned();
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.open_picked_project(&target, path, window, cx);
                    });
                }
            }
            Ok(Ok(None)) => {}
            error => {
                let _ = this.update(cx, |this, cx| {
                    this.error = Some(format!("Could not open folder: {error:?}"));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn project_picker_target(&self) -> String {
        self.vorkspace_dialog
            .as_ref()
            .map(|d| d.editing.clone())
            .unwrap_or_else(|| self.workspace.active_vorkspace.clone())
    }

    fn open_picked_project(
        &mut self,
        target: &str,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.workspace.vorkspaces.iter().any(|v| v.id == target) {
            self.error = Some("That vorkspace was removed. Open the folder again.".into());
            cx.notify();
            return;
        }
        if !self.allow_project_entry(&path, cx) {
            return;
        }
        self.workspace.add_project_to_vorkspace(target, &path);
        if target == self.workspace.active_vorkspace {
            self.open_project_from_user(path, window, cx);
        } else {
            self.persist(cx);
            cx.notify();
        }
    }
    fn create(&mut self, kind: Kind, cx: &mut Context<Self>) {
        if self.management_modal_focus(cx).is_some() {
            return;
        }
        if self.workspace.project.is_empty() {
            self.open_project(cx);
        } else {
            self.create_session(kind, false, cx);
        }
    }
    fn create_session(&mut self, kind: Kind, only_if_empty: bool, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let cwd = self.workspace.project.clone();
        if cwd.is_empty() {
            return;
        }
        let selected = self.worktree_focus.get(&cwd).cloned();
        let focus = if only_if_empty {
            selected.clone()
        } else {
            let checkout = self.focused_checkout(cx);
            let key = vorkspace_core::files::path_key(checkout);
            (key != vorkspace_core::files::path_key(&cwd)).then(|| {
                selected
                    .as_ref()
                    .filter(|focus| vorkspace_core::files::path_key(&focus.path) == key)
                    .cloned()
                    .unwrap_or_else(|| crate::git_tools::worktree_switcher::WorktreeFocus {
                        path: checkout.into(),
                        branch: None,
                    })
            })
        };
        let blank_key = format!("{cwd}\0{}", focus.as_ref().map_or("", |f| f.path.as_str()));
        if only_if_empty && !self.pending_project_blanks.insert(blank_key.clone()) {
            return;
        }
        let creation_key = vorkspace_core::files::path_key(&cwd);
        *self
            .pending_project_creations
            .entry(creation_key.clone())
            .or_default() += 1;
        let model = self
            .workspace
            .preferences
            .default_models
            .get(kind.id())
            .cloned();
        let navigation_revision = self.navigation_revision;
        let account = crate::accounts::get(cx)
            .map(|center| center.read(cx).selected(kind, &cwd))
            .unwrap_or_default();
        let access = crate::conversation::default_access(cx);
        let effort = crate::conversation::preferred_effort(kind.id(), cx);
        let mut install = self.session_install(cx);
        let work = cx.background_executor().spawn({
            let cwd = cwd.clone();
            let worktree_cwd = focus.as_ref().map(|f| f.path.clone());
            async move {
                let store = locked(&store)?;
                let summary = store.create(kind.id(), &cwd)?;
                install.track(summary.id);
                let mut settings = session_loading::new_session_settings(
                    store
                        .desktop_preferences()
                        .and_then(|preferences| preferences.session_defaults(kind.id(), &cwd)),
                    model,
                );
                settings.access = access;
                if let Some(effort) = effort {
                    settings.model_settings.retain(|key, _| {
                        !matches!(
                            key.as_str(),
                            "effort" | "reasoning" | "reasoningEffort" | "reasoning_effort"
                        )
                    });
                    settings.effort = Some(effort);
                }
                settings.account = account;
                settings.worktree_cwd = worktree_cwd;
                if settings.model.is_some()
                    || settings.effort.is_some()
                    || !settings.model_settings.is_empty()
                    || settings.worktree_cwd.is_some()
                    || settings.account.id != vorkspace_core::accounts::DEFAULT
                    || access != Default::default()
                {
                    store.save_agent_settings(summary.id, &settings)?;
                }
                Ok::<_, String>((summary, settings, install))
            }
        });
        let previous = self.new_tab_task.take();
        self.new_tab_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            if let Some(previous) = previous {
                previous.await;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_project_creation(&creation_key);
                if only_if_empty {
                    this.pending_project_blanks.remove(&blank_key);
                }
                match result {
                    Ok((summary, settings, mut install)) => {
                        let id = summary.id;
                        let Some(view) = this.install_prepared(summary, &install, cx) else {
                            this.session_install_failed(cx);
                            return;
                        };
                        view.update(cx, |view, cx| view.seed_new_settings(settings, cx));
                        this.seed_worktree(id, focus.as_ref(), cx);
                        this.publish_prepared(&view, Some(&mut install), cx);
                        // A pending blank belongs to the copy that requested it;
                        // navigation to another copy must never redirect that request.
                        if this.workspace.project != cwd
                            || if only_if_empty {
                                this.worktree_focus.get(&cwd) != selected.as_ref()
                                    || this.workspace.active.is_some()
                                    || this.bot_active(cx)
                                    || this.app_view_open()
                                    || this.settings.is_some()
                            } else {
                                this.navigation_revision != navigation_revision
                            }
                        {
                            this.invalidate_chrome(cx);
                            cx.notify();
                            return;
                        }
                        this.order.retain(|s| *s != id);
                        this.order.insert(0, id);
                        this.activate_session(id, window, cx);
                    }
                    Err(error) => {
                        this.error = Some(error);
                        cx.notify();
                    }
                }
            });
        }));
        cx.notify();
    }
}
/// Highlight only the existing hairline; its parent keeps the wider, transparent
/// grab area centered on that line. The wash brightens the underlying border.
fn resize_border(vertical: bool, dragging: bool) -> gpui::Div {
    div()
        .absolute()
        .bg(content(0.12))
        .opacity(if dragging { 1. } else { 0. })
        .group_hover("resize-handle", |s| s.opacity(1.))
        .map(|d| {
            if vertical {
                d.top_0()
                    .h_full()
                    .left(gpui::relative(0.5))
                    .ml(px(-0.5))
                    .w(px(1.))
            } else {
                d.left_0()
                    .w_full()
                    .top(gpui::relative(0.5))
                    .mt(px(-0.5))
                    .h(px(1.))
            }
        })
}

/// paneEnterFrom/collapsedRect: a pane on an outer edge slides in from it;
/// one in the middle opens from its centre line.
fn enter_from(leaf: &LayoutLeaf) -> (Edge, Rect) {
    let r = leaf.rect;
    let edge = 0.001;
    if leaf.axis == SplitDir::Right {
        if r.x + r.w >= 1. - edge {
            return (
                Edge::Right,
                Rect {
                    x: r.x + r.w,
                    w: 0.,
                    ..r
                },
            );
        }
        if r.x <= edge {
            return (Edge::Left, Rect { w: 0., ..r });
        }
        (
            Edge::Right,
            Rect {
                x: r.x + r.w / 2.,
                w: 0.,
                ..r
            },
        )
    } else {
        if r.y + r.h >= 1. - edge {
            return (
                Edge::Bottom,
                Rect {
                    y: r.y + r.h,
                    h: 0.,
                    ..r
                },
            );
        }
        if r.y <= edge {
            return (Edge::Top, Rect { h: 0., ..r });
        }
        (
            Edge::Bottom,
            Rect {
                y: r.y + r.h / 2.,
                h: 0.,
                ..r
            },
        )
    }
}
impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scale = self.workspace.preferences.interface_scale();
        if window.ui_scale() != scale {
            window.set_ui_scale(scale, cx);
            self.close_preview(cx);
            self.invalidate_chrome(cx);
        }
        // Use the scale of this frame, which can differ from a zoom preference
        // deferred by GPUI until the next frame. This covers restored, menu,
        // keyboard, Settings and shared-preference zoom changes alike.
        vorkspace_ui::traffic_lights::pin(window);
        self.keep_visible_idle_agents(cx);
        self.tick_resize_settle(window);
        if self.management_modal_focus(cx).is_none()
            && let Some(id) = self.pending_composer_focus.take()
            && self
                .workspace
                .active
                .and_then(|pane| self.workspace.session_for_pane(pane))
                == Some(id)
            && let Some(view) = self.sessions.get(&id)
        {
            self.review_focused = false;
            if view.read(cx).user_read_only() {
                window.focus(&self.focus, cx);
            } else {
                window.focus(&view.read(cx).input.focus_handle(cx), cx);
            }
        }
        if let Some(m) = &self.bots {
            m.update(cx, |m, _| m.bind_window(window));
        }
        self.render_passes += 1;
        if let Some((id, tab)) = self.pending_review.take()
            && self.workspace.tab_of(id).is_some()
        {
            self.focus_pane(id, window, cx);
            self.open_review(tab, window, cx);
        }
        // Native fullscreen transitions resize the window, which redraws it.
        self.fullscreen = window.is_fullscreen();
        let p = &self.workspace.preferences;
        let system_light = matches!(
            cx.window_appearance(),
            gpui::WindowAppearance::Light | gpui::WindowAppearance::VibrantLight
        );
        if configure(p, system_light, true) {
            cx.refresh_windows();
        }
        let glass = window_glass(p, system_light, true);
        let blur = if glass { p.sidebar_blur.min(100) } else { 0 };
        if self.window_glass != Some(glass) || self.window_blur != Some(blur) {
            self.window_glass = Some(glass);
            self.window_blur = Some(blur);
            vorkspace_macos::window_appearance::apply(window, glass, blur);
        }
        self.sync_settings(cx);
        self.sidebar_fraction = match &self.sidebar_slide {
            Some(slide) => {
                let (fraction, done) = slide.at(vorkspace_ui::clock::Instant::now());
                if done {
                    self.sidebar_slide = None;
                } else {
                    window.request_animation_frame();
                }
                fraction
            }
            None => {
                if self.workspace.sidebar_visible {
                    1.
                } else {
                    0.
                }
            }
        };
        if self.settings.is_some() {
            self.sidebar_fraction = 0.;
        }
        let room = (window.viewport_size().width.as_f32() - self.classic_rail_width()).max(0.);
        let width = self
            .workspace
            .right_panel_width
            .min((room - 252.).max(room * 0.31));
        if self.sidebar_content_width != width {
            self.sidebar_content_width = width;
            self.invalidate_chrome(cx);
        }
        let sidebar_width = self.docked_sidebar_width();
        let name = self
            .workspace
            .active_vorkspace()
            .map_or("vorkspace", |def| def.name.as_str());
        if self.native_title != name {
            self.native_title = name.to_owned();
            window.set_window_title(name);
        }
        if !self.window_observed {
            self.window_observed = true;
            self.native_window = window.window_handle().downcast();
            crate::menu_bar::remember(window, cx);
            self.shortcuts = Some(shortcuts::install(cx.weak_entity(), window, cx));
            self.observations
                .push(cx.on_focus_lost(window, |this, window, cx| {
                    // Cached editors and previews can outlive their rendered pane.
                    // Recover as soon as its focus node leaves the tree, including
                    // close paths without a Window in their original event.
                    let target = window
                        .focus_lost_restore_target(cx)
                        .unwrap_or_else(|| this.focus.clone());
                    window.focus(&target, cx);
                }));
            self.window_active = window.is_window_active();
            self.observations
                .push(cx.observe_window_activation(window, |this, window, cx| {
                    cx.set_reduce_motion(vorkspace_macos::accessibility::reduce_motion());
                    this.window_active = window.is_window_active();
                    if this.window_active {
                        crate::menu_bar::remember(window, cx);
                        let _ = this.save_window_state(None, false, cx);
                        for id in &this.visible_chats {
                            if let Some(view) = this.sessions.get(id) {
                                view.update(cx, |v, cx| v.refresh_if_stale(cx));
                            }
                        }
                        this.refresh_notification_permission(cx);
                    }
                    cx.notify();
                }));
            self.start_native_notifications(cx);
            let shell = cx.entity().downgrade();
            window.on_window_should_close(cx, move |window, cx| {
                shell
                    .update(cx, |shell, cx| shell.allow_window_close(window, cx))
                    .unwrap_or(true)
            });
            self.observations
                .push(cx.observe_window_activation(window, |this, window, cx| {
                    if !window.is_window_active() {
                        this.close_preview(cx);
                    } else {
                        this.resume_git(cx);
                    }
                }));
            self.observations
                .push(cx.observe_window_bounds(window, |this, window, cx| {
                    this.close_preview(cx);
                    let size = window.viewport_size();
                    if let Some(mut bounds) = this.pane_viewport.get() {
                        bounds.size.width = px((bounds.size.width.as_f32() + size.width.as_f32()
                            - this.viewport_width)
                            .max(0.));
                        bounds.size.height = px((bounds.size.height.as_f32()
                            + size.height.as_f32()
                            - this.viewport_height)
                            .max(0.));
                        this.pane_viewport.set(Some(bounds));
                    }
                    this.viewport_width = size.width.as_f32();
                    this.viewport_height = size.height.as_f32();
                    cx.notify();
                }));
        }
        self.viewport_width = window.viewport_size().width.as_f32();
        self.viewport_height = window.viewport_size().height.as_f32();
        self.ensure_right_panel(window, cx);
        let poll_git = self.window_active && !self.bot_active(cx);
        self.git.update(cx, |git, cx| {
            git.set_polling(poll_git && git.available(), cx)
        });
        if let Some(panel) = &self.right_panel {
            panel.update(cx, |panel, cx| {
                panel.set_show_excluded(self.workspace.preferences.show_excluded_files, cx);
            });
        }
        self.sync_right_panel(cx);
        self.sync_notices(cx);
        let management_open = self.management_modal_focus(cx).is_some();
        let background = self.background_modal(window, cx);
        let (workspace_background, management_background) = if management_open {
            (None, background)
        } else {
            (background, None)
        };
        let workspace = div()
            .id("vorkspace-workspace")
            .role(Role::Application)
            .aria_label("vorkspace Native")
            .when(!management_open, |d| d.key_context("Workspace"))
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::open_image))
            .on_action(
                cx.listener(|this, open: &crate::diagrams::Open, window, cx| {
                    this.open_image(
                        &crate::image_lightbox::Open {
                            source: crate::image_lightbox::Source::Diagram {
                                kind: open.kind,
                                source: open.source.clone(),
                                natural: open.natural,
                            },
                            alt: open.alt.clone(),
                            draft_session: None,
                        },
                        window,
                        cx,
                    );
                }),
            )
            .flex()
            .flex_col()
            .size_full()
            .bg(shell_fill())
            .text_color(content(1.))
            .font_family(vorkspace_ui::fonts::UI)
            .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if this.session_preview.as_ref().is_some_and(|preview| {
                    !preview
                        .surface
                        .get()
                        .is_some_and(|bounds| bounds.contains(&event.position))
                }) {
                    this.close_preview(cx);
                }
            }))
            .on_action(
                cx.listener(|this, event: &crate::file_link::OpenPath, window, cx| {
                    let cwd = this.git.read(cx).project().to_owned();
                    let (path, location) = crate::file_link::file_location(&event.0, &cwd);
                    this.open_review(
                        crate::git_tools::review::ReviewTab::File { cwd, path },
                        window,
                        cx,
                    );
                    if let Some((line, column)) = location
                        && let Some(pane) = this.active_review(cx)
                    {
                        pane.update(cx, |pane, cx| pane.navigate_file(line, column, cx));
                    }
                }),
            )
            .on_action(
                cx.listener(|this, _: &GoToFile, window, cx| {
                    this.open_quick_open(false, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &OpenCommandPalette, window, cx| {
                this.open_quick_open(true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowInbox, window, cx| this.open_inbox(window, cx)))
            .on_action(cx.listener(|this, _: &ShowGitHub, window, cx| {
                this.open_connector(Connector::Github, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowGitLab, window, cx| {
                this.open_connector(Connector::Gitlab, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowJira, window, cx| {
                this.open_connector(Connector::Jira, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowNotes, window, cx| this.open_notes(window, cx)))
            .on_action(cx.listener(|this, _: &ShowAutomations, window, cx| {
                this.open_automations(window, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenAppearance, window, cx| {
                this.open_settings(crate::settings::Section::Appearance, window, cx)
            }))
            .on_action(|_: &MinimizeWindow, window, _| window.minimize_window())
            .on_action(|_: &ZoomWindow, window, _| window.zoom_window())
            .on_action(|_: &ToggleFullScreen, window, _| window.toggle_fullscreen())
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
                this.open_settings(crate::settings::Section::General, window, cx)
            }))
            .on_action(|_: &crate::background_helper::Toggle, _, cx| {
                crate::background_helper::toggle(cx)
            })
            .on_action(
                cx.listener(|this, _: &session_groups::RenameSidebarItem, window, cx| {
                    this.rename_sidebar_item(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &NewCodex, window, cx| {
                // ⌘T in the focused dock opens another terminal there.
                if this.dock_focused(window, cx).is_some() {
                    this.new_terminal(window, cx);
                } else {
                    this.create(this.default_kind(), cx);
                }
            }))
            .on_action(cx.listener(
                |this, _: &crate::file_tools::file_editor::ToggleAutosave, _, cx| {
                    this.workspace.preferences.autosave = !this.workspace.preferences.autosave;
                    this.apply_preferences(cx);
                    this.persist(cx);
                    cx.notify();
                },
            ))
            .on_action(cx.listener(|this, _: &NewClaude, _, cx| this.create(Kind::Claude, cx)))
            .on_action(cx.listener(|this, _: &OpenProject, _, cx| this.open_project(cx)))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                if this.settings.is_some() {
                    this.close_settings(window, cx);
                    return;
                }
                // ⌘W in the focused dock closes its terminal.
                if let Some(ix) = this.dock_focused(window, cx) {
                    this.close_terminal(ix, window, cx);
                    return;
                }
                // A focused review closes its tab first, as a focused file pane does.
                if this.review_focused
                    && let Some(review) = this.active_review(cx)
                {
                    review.update(cx, |r, cx| r.close_active(cx));
                    return;
                }
                if let Some(id) = this.workspace.active {
                    // Close Pane (⌘W) in a split; the whole tab otherwise.
                    this.close_pane(id, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| this.cycle(1, window, cx)))
            .on_action(cx.listener(|this, _: &PreviousTab, window, cx| this.cycle(-1, window, cx)))
            .on_action(cx.listener(|this, _: &VisitBack, window, cx| this.visit(-1, window, cx)))
            .on_action(cx.listener(|this, _: &VisitForward, window, cx| this.visit(1, window, cx)))
            .on_action(cx.listener(|this, _: &CloseAllTabs, window, cx| {
                // An empty strip can still have a new-tab request in flight.
                this.navigation_revision += 1;
                this.close_title_tabs(&this.visible_tabs(cx), None, window, cx);
                if !this.bot_active(cx) {
                    this.ensure_project_session(window, cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &CloseOtherTabs, window, cx| {
                    this.close_other_tabs(window, cx)
                }),
            )
            .on_action(cx.listener(|this, action: &ActivateTab, window, cx| {
                this.activate_tab(action.0, window, cx)
            }))
            .on_action(cx.listener(|this, _: &PreviousSession, window, cx| {
                this.step_session(-1, false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &NextSession, window, cx| {
                this.step_session(1, false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &PreviousSessionInTab, window, cx| {
                this.step_session(-1, true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &NextSessionInTab, window, cx| {
                this.step_session(1, true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &PreviousProject, window, cx| {
                this.step_project(-1, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &NextProject, window, cx| this.step_project(1, window, cx)),
            )
            .on_action(cx.listener(|this, _: &SwitchModel, window, cx| {
                if let Some(view) = this.workspace.active.and_then(|id| this.sessions.get(&id)) {
                    view.update(cx, |v, cx| v.toggle_recent_models(window, cx));
                }
            }))
            .on_action(
                cx.listener(|this, _: &vorkspace_ui::actions::DismissMenu, window, cx| {
                    if this.remote_project_picker.is_some() {
                        this.close_remote_project(window, cx);
                        return;
                    }
                    if this.quick_open.is_some() {
                        this.close_quick_open(window, cx);
                        return;
                    }
                    if this.app_search.is_some() {
                        this.close_app_search(window, cx);
                        return;
                    }
                    if this.link_dialog.is_some() {
                        this.close_link_dialog(window, cx);
                        return;
                    }
                    if this.background_dialog.is_some() {
                        this.close_background_dialog(window, cx);
                        return;
                    }
                    if this.vorkspace_dialog.is_some() {
                        this.close_vorkspace_dialog(window, cx);
                        return;
                    }
                    if let Some(settings) = this.settings.clone() {
                        if !settings.update(cx, |s, cx| s.dismiss_transient(window, cx)) {
                            this.close_settings(window, cx);
                        }
                        return;
                    }
                    if let Some(bots) = this.bots.clone()
                        && bots.update(cx, |m, cx| m.dismiss_settings(window, cx))
                    {
                        return;
                    }
                    if this.app_view_open() {
                        this.close_app_view(window, cx);
                        return;
                    }
                    if this.session_preview.is_some() {
                        this.close_preview(cx);
                        return;
                    }
                    if this.rail_search_open {
                        this.close_rail_search(window, cx);
                        return;
                    }
                    if this.menu.is_none()
                        && this.workspace.sidebar_visible
                        && !this.bot_active(cx)
                        && this.classic_tool_tab() == Some(crate::panel_kit::RightTab::Files)
                        && let Some(panel) = &this.right_panel
                    {
                        let explorer = panel.read(cx).explorer.clone();
                        if explorer.update(cx, |explorer, cx| explorer.close_search(window, cx)) {
                            cx.stop_propagation();
                            return;
                        }
                    }
                    // `Session: Stop` when no menu is open to close.
                    if this.menu.is_none()
                        && let Some(view) =
                            this.workspace.active.and_then(|id| this.sessions.get(&id))
                    {
                        view.update(cx, |v, cx| v.stop_if_busy(cx));
                    }
                    this.close_menu(cx);
                    this.tab_drag = None;
                    this.cancel_pane_drag(cx);
                    this.pane_sash = None;
                    this.invalidate_chrome(cx);
                    cx.notify();
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::git_tools::diff_view::Unified, _, cx| {
                    this.workspace.preferences.diff_view = "unified".into();
                    this.apply_preferences(cx);
                    this.persist(cx);
                    cx.notify();
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::git_tools::diff_view::SideBySide, _, cx| {
                    this.workspace.preferences.diff_view = "split".into();
                    this.apply_preferences(cx);
                    this.persist(cx);
                    cx.notify();
                }),
            )
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| this.toggle_project_rail(cx)))
            .on_action(
                cx.listener(|this, _: &ToggleRightPanel, window, cx| {
                    this.toggle_sidebar(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &SearchFiles, window, cx| {
                if this.app_view_open() || this.settings.is_some() || this.bot_active(cx) {
                    return;
                }
                this.close_app_search(window, cx);
                this.select_classic_sidebar(crate::workspace::SidebarTab::Files, window, cx);
                if let Some(panel) = &this.right_panel {
                    let explorer = panel.read(cx).explorer.clone();
                    explorer.update(cx, |explorer, cx| explorer.open_search(window, cx));
                }
            }))
            .on_action(
                cx.listener(|this, _: &FocusSearch, window, cx| this.focus_search(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ToggleTerminal, window, cx| {
                    this.toggle_terminal(window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &NewTerminal, window, cx| this.new_terminal(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SplitRight, _, cx| this.split_session(SplitDir::Right, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SplitDown, _, cx| this.split_session(SplitDir::Down, cx)),
            )
            .on_action(cx.listener(|this, _: &FocusPaneLeft, window, cx| {
                this.focus_neighbor(Edge::Left, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusPaneRight, window, cx| {
                this.focus_neighbor(Edge::Right, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusPaneUp, window, cx| {
                this.focus_neighbor(Edge::Top, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusPaneDown, window, cx| {
                this.focus_neighbor(Edge::Bottom, window, cx)
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if let Some(bots) = &this.bots {
                    bots.update(cx, |m, cx| m.move_rail_drag(event, cx));
                }
                this.move_tab_drag(event, cx);
                this.move_dock_drag(event, cx);
                this.move_pane_drag(event, cx);
                this.drag_sash(event, cx);
                this.drag_dock(event, cx);
                this.drag_review(event, cx);
                if let Some((start, width)) = this.project_rail_drag {
                    if event.pressed_button == Some(MouseButton::Left) {
                        let width = width + (event.position.x - start).as_f32();
                        this.drag_resize(ResizeTarget::ProjectRail, width, cx);
                        this.project_rail_frame.update(cx, |_, cx| cx.notify());
                    } else {
                        this.release_resizes(cx);
                    }
                }
                if let Some((start, width)) = this.resize {
                    if event.pressed_button == Some(MouseButton::Left) {
                        let delta = (event.position.x - start).as_f32();
                        let width = width + delta;
                        this.drag_resize(ResizeTarget::Sidebar, width, cx);
                    } else {
                        this.release_resizes(cx);
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.finish_tab_drag(window, cx);
                    this.finish_dock_drag(cx);
                    this.finish_pane_drag(window, cx);
                    if let Some(m) = this.bots.clone() {
                        m.update(cx, |m, cx| m.finish_reorder(cx));
                    }
                    this.release_resizes(cx);
                    if std::mem::take(&mut this.review_drag)
                        && let Some(review) = this.active_review(cx)
                    {
                        review.update(cx, |_, cx| {
                            cx.emit(crate::git_tools::review::ReviewEvent::Changed)
                        });
                    }
                }),
            )
            .relative()
            .child(self.layout_body(sidebar_width, window, cx))
            .children(self.classic_session_preview(cx))
            .child(self.notices_overlay(cx))
            .children(self.menu.as_ref().map(|open| {
                let point = matches!(open.anchor, Anchor::Point(_));
                let beside = matches!(open.target, MenuTarget::Project { .. });
                // Context menus open at the pointer, as ExplorerMenu
                // does; TabGroupMenu hangs its top-left corner there.
                menu_popover(&open.menu, open.anchor.clone(), cx)
                    .when(point, |p| p.gap(0.))
                    .when(beside, |p| p.side(Side::Right))
                    .when(open.target == MenuTarget::SidebarOptions, |p| {
                        p.align(Align::End)
                    })
            }))
            .children(self.drag_ghost(cx))
            .children(workspace_background)
            .children(
                self.bots
                    .clone()
                    .and_then(|bots| bots.read(cx).creation_surface(window)),
            )
            .children(self.link_modal(window, cx))
            .children(self.resource_modal(window, cx))
            .children(self.quick_open_modal(window, cx))
            .children(self.remote_project_modal(window, cx))
            .children(self.notice_modals(window, cx))
            .children(crate::accounts::get(cx))
            .children(
                self.image_lightbox
                    .as_ref()
                    .map(|(view, _)| crate::image_lightbox::overlay(view)),
            );
        // Management is a sibling of the workspace so native menu actions
        // and custom shortcuts cannot bubble into the obscured workspace.
        div()
            .relative()
            .size_full()
            .when(management_open, |d| {
                d.key_context("VorkspaceModal").on_action(cx.listener(
                    |this, _: &vorkspace_ui::actions::DismissMenu, window, cx| {
                        if this.background_dialog.is_some() {
                            this.close_background_dialog(window, cx);
                        } else {
                            this.close_vorkspace_dialog(window, cx);
                        }
                    },
                ))
            })
            .on_action(
                cx.listener(|this, _: &crate::Quit, window, cx| this.request_quit(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &crate::Restart, window, cx| {
                    this.request_restart(window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::image_lightbox::ZoomIn, window, cx| {
                    this.scale_interface(10, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::image_lightbox::ZoomOut, window, cx| {
                    this.scale_interface(-10, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::image_lightbox::ResetZoom, window, cx| {
                    this.scale_interface(0, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &FocusNext, window, cx| {
                if let Some(scope) = this.management_modal_focus(cx) {
                    window.focus_tab_within(&scope, false, cx);
                } else {
                    window.focus_next(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &FocusPrevious, window, cx| {
                if let Some(scope) = this.management_modal_focus(cx) {
                    window.focus_tab_within(&scope, true, cx);
                } else {
                    window.focus_prev(cx);
                }
            }))
            .child(workspace)
            .children(management_background)
            .children(self.vorkspace_modal(window, cx))
    }
}

#[cfg(test)]
mod tests;

pub(crate) fn workspace_target(cx: &App) -> Option<gpui::AnyWindowHandle> {
    let windows = cx.windows();
    let live = |handle: &gpui::AnyWindowHandle| {
        handle.downcast::<Shell>().is_some() && windows.contains(handle)
    };
    cx.active_window()
        .filter(live)
        .or_else(|| {
            cx.try_global::<crate::menu_bar::WorkspaceTarget>()
                .and_then(|target| target.0)
                .filter(live)
        })
        .or_else(|| cx.windows().into_iter().find(live))
}

#[cfg(test)]
mod app_menu_tests {
    use crate::{app_menu::build, settings::Preferences};
    use gpui::MenuItem;
    use gpui::{TestAppContext, VisualTestContext};

    #[gpui::test]
    fn menus_restore_native_commands_and_share_live_workspace_owners(cx: &mut TestAppContext) {
        cx.update(crate::app_menu::init);
        let (window, _, _) = crate::shell::tests::window(cx);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let menus = build(&Preferences::default());
        assert_eq!(
            menus
                .iter()
                .map(|menu| menu.name.as_ref())
                .collect::<Vec<_>>(),
            ["vorkspace", "File", "Edit", "View", "Window", "Help"]
        );
        let action = |label: &str| {
            menus
                .iter()
                .flat_map(|menu| &menu.items)
                .find_map(|item| match item {
                    MenuItem::Action { name, action, .. } if name == label => {
                        Some(action.boxed_clone())
                    }
                    _ => None,
                })
                .unwrap()
        };
        for label in ["GitHub", "Notes", "Appearance…"] {
            let command = action(label);
            window
                .update(visual, |_, native, cx| {
                    let orphan = cx.focus_handle();
                    native.focus(&orphan, cx);
                })
                .unwrap();
            // Native menus enter through App, outside any borrowed window.
            visual.cx.update(|cx| cx.dispatch_action(command.as_ref()));
            visual.run_until_parked();
            // The sidebar is the user's toggle; only Settings covers it.
            assert_eq!(
                visual.debug_bounds("sidebar-resize").is_none(),
                label == "Appearance…",
                "{label}"
            );
            let state = window
                .read_with(visual, |shell, cx| {
                    (
                        shell.active_app_view,
                        shell.settings.as_ref().map(|view| view.read(cx).section),
                        shell.app_search.is_some(),
                    )
                })
                .unwrap();
            let header = visual.debug_bounds("settings-page-header");
            let settings = visual.debug_bounds("settings-view");
            let nav = visual.debug_bounds("settings-nav");
            assert!(
                if label == "Appearance…" {
                    header.is_some()
                } else {
                    visual.debug_bounds("sidebar-content").is_some()
                },
                "{label}: state={state:?}, settings={settings:?}, nav={nav:?}, header={header:?}"
            );
        }
        // Menu and keyboard scaling use the same persisted preference owner.
        for label in ["Zoom In", "Zoom Out", "Reset Zoom"] {
            let command = action(label);
            // Native menus enter through App, outside any borrowed window.
            visual.cx.update(|cx| cx.dispatch_action(command.as_ref()));
            visual.run_until_parked();
            window
                .update(visual, |shell, native, _| {
                    let expected = match label {
                        "Zoom In" => 110,
                        _ => 100,
                    };
                    assert_eq!(shell.workspace.preferences.ui_scale, expected);
                    assert!((native.ui_scale() - expected as f32 / 100.).abs() < f32::EPSILON);
                })
                .unwrap();
        }
        window
            .update(visual, |shell, _, _| {
                assert_eq!(shell.workspace.preferences.ui_scale, 100)
            })
            .unwrap();
    }
    #[gpui::test]
    fn workspace_menu_targets_one_live_workspace_while_another_window_is_active(
        cx: &mut TestAppContext,
    ) {
        use gpui::AppContext;
        cx.update(crate::app_menu::init);
        let (workspace, _, _) = crate::shell::tests::window(cx);
        let secondary = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| cx.new(crate::input::Input::new))
                .unwrap()
        });
        secondary
            .update(cx, |_, window, _| window.activate_window())
            .unwrap();
        cx.run_until_parked();
        cx.update(|cx| cx.dispatch_action(&super::ShowInbox));
        cx.run_until_parked();
        workspace
            .update(cx, |shell, _, _| {
                assert_eq!(
                    shell.active_app_view,
                    Some(crate::workspace::AppView::Connector(
                        crate::workspace::Connector::Github
                    ))
                );
            })
            .unwrap();
        assert!(secondary.read_with(cx, |_, _| ()).is_ok());
    }
}
