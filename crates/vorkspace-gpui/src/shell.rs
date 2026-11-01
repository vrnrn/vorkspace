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
pub(crate) use crate::bots::host_api::AppStart;
mod app_views;
mod automation_runs;
pub(crate) mod bots;
mod classic_rail;
mod classic_sidebar;
mod external_editor;
mod file_panes;
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
mod rails;
mod remote_projects;
mod remote_sessions;
mod search;
mod session_filters;
mod session_folders;
mod session_groups;
mod session_loading;
mod session_preview;
mod settings;
mod shortcuts;
mod sidebar_list;
mod sidebar_peek;
mod status_menu;
mod terminal_dock;
mod terminal_tabs;
mod title_tabs;
#[cfg(test)]
mod titlebar_tests;
#[cfg(test)]
mod ui_parity_tests;
mod vox;
mod worktrees;

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
    right_panel::{PanelEvent, RightPanel},
    terminal::{Terminal, TerminalEvent},
    usage_footer::UsageFooter,
    workspace::{
        Connector, DockSide, SidebarNavigation, TitleTab, VisitHistory, Workspace, project_name,
    },
};
use gpui::{
    AnyElement, App, Bounds, BoxShadow, ClipboardItem, Context, CursorStyle, DispatchPhase, Entity,
    FocusHandle, Focusable, FontWeight, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PathPromptOptions, Pixels, Point, Role, ScrollHandle, SharedString,
    StyleRefinement, Subscription, Window, actions, canvas, div, prelude::*, px, rgb, rgba,
};
use session_groups::local_reminder_label;
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
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
    session_folders::bind(cx);
    let modifier = "cmd";
    // Scoped to the shell, so the focused composer, menus and pickers outrank
    // these: GPUI ranks an unscoped binding above every context.
    let context = Some("Workspace");
    cx.bind_keys([
        KeyBinding::new(
            &format!("{modifier}-="),
            crate::image_lightbox::ZoomIn,
            context,
        ),
        KeyBinding::new(
            &format!("{modifier}-+"),
            crate::image_lightbox::ZoomIn,
            context,
        ),
        KeyBinding::new(
            &format!("{modifier}--"),
            crate::image_lightbox::ZoomOut,
            context,
        ),
        KeyBinding::new(
            &format!("{modifier}-0"),
            crate::image_lightbox::ResetZoom,
            context,
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
        KeyBinding::new("escape", vorkspace_ui::actions::DismissMenu, context),
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
        KeyBinding::new("tab", FocusNext, context),
        KeyBinding::new("shift-tab", FocusPrevious, context),
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
    TerminalTitleTab(TitleTab),
    Session(i64),
    Folder(String),
    /// The sidebar project header's menu.
    Header,
    SessionFilter,
    SidebarOptions,
    /// A project's actions; `rail` adds the project rail's grouping and pinning.
    Project {
        path: String,
        rail: bool,
    },
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
const CORNER_TABS_LEAD: f32 = 7.;
const CORNER_CONTROL_GAP: f32 = 4.;
const CORNER_BUTTONS_WIDTH: f32 = 48. + 2. * CORNER_CONTROL_GAP;
// TitleBar.tsx's labels, with their shortcuts.
const TOGGLE_SIDEBAR_LABEL: &str = "Toggle Sidebar (⌘B)";
const BACK_LABEL: &str = "Back (⌘[)";
const FORWARD_LABEL: &str = "Forward (⌘])";
const TOGGLE_RIGHT_PANEL_LABEL: &str = "Toggle right sidebar (⌘⇧B)";
const TOGGLE_RIGHT_PANEL_DOT_LABEL: &str = "Toggle right sidebar (⌘⇧B, uncommitted changes)";
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
    RightPanel,
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
const PANE_LIMIT_MESSAGE: &str = "Not enough room to tile here. Widen the window or use another tab; columns can contain up to three sessions.";
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
    #[cfg(not(test))]
    status_menu: Option<vorkspace_macos::status_menu::Status>,
    bots: Option<Entity<crate::bots::Bots>>,
    vox: Option<Entity<crate::vox::Vox>>,
    quick_open: Option<quick_open::Open>,
    image_lightbox: Option<(Entity<crate::image_lightbox::ImageLightbox>, Subscription)>,
    app_bridge: Option<vorkspace_core::bot::bridge::Server>,
    orchestrations: HashMap<i64, orchestration::Run>,
    /// Each lead's last orchestration, whose agents hang under it in the sidebar.
    orchestration_summaries: HashMap<i64, vorkspace_core::orchestration::Summary>,
    /// Sessions an /operator thread started, by the session that started them.
    started_by: HashMap<i64, i64>,
    /// The sidebar rails under orchestrators and /operator threads.
    rails: rails::Rails,
    usage_footer: Entity<UsageFooter>,
    /// Resource pressure indicator and its running-work dialog.
    resources: Entity<ResourceMonitor>,
    sidebar_frame: Entity<Chrome>,
    project_rail_frame: Entity<Chrome>,
    project_rail_drag: Option<(Pixels, f32)>,
    sidebar_search: Entity<input::Input>,
    sidebar_list: sidebar_list::SidebarList,
    sidebar_slide: Option<PaneSlide>,
    sidebar_fraction: f32,
    /// The collapsed inset sidebar peeking out over the workspace.
    sidebar_peek: Option<sidebar_peek::SidebarPeek>,
    peek_animating: bool,
    peek_close: Option<gpui::Task<()>>,
    peek_toggle_bounds: AnchorSlot,
    /// useTrafficLightsVisible: fullscreen hides the native buttons, so the
    /// title bar takes their room back.
    fullscreen: bool,
    tabs_frame: Entity<Chrome>,
    store: Database,
    remote: Arc<vorkspace_core::remote::Connections>,
    remote_project_picker: Option<remote_projects::Open>,
    remote_refreshing: bool,
    remote_marks: HashMap<i64, (bool, bool)>,
    _remote_clock: gpui::Task<()>,
    sessions: HashMap<i64, Entity<Conversation>>,
    /// Lightweight history index; only opened or previewed chats own GPUI entities.
    session_summaries: HashMap<i64, Session>,
    draft_sessions: HashSet<i64>,
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
    groups: HashMap<String, vorkspace_core::session_groups::Groups>,
    reminders: Vec<vorkspace_core::store::Reminder>,
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
    folder_bounds: HashMap<String, AnchorSlot>,
    sidebar_drop: Option<session_groups::ListDrop>,
    folder_drag: Option<session_groups::FolderDrag>,
    skip_folder_click: bool,
    filter_anchor: AnchorSlot,
    folder_picker: Option<session_folders::Open>,
    pending_folder: Option<(i64, AnchorSlot)>,
    _session_clock: gpui::Task<()>,
    notes: Option<Entity<Notes>>,
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
    saved_search: Option<(String, vorkspace_core::search::Scope)>,
    order: Vec<i64>,
    pub(crate) workspace: Workspace,
    external_editors: Option<Vec<vorkspace_core::external_editor::ExternalEditor>>,
    external_editor_scan: u64,
    deleting_projects: HashSet<String>,
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
    backgrounds: HashMap<i64, Entity<crate::project_background::ProjectBackground>>,
    background_card: AnchorSlot,
    liquid: crate::liquid::Liquid,
    /// The title bar's tabs, rising from the line beneath them.
    tab_liquid: crate::liquid::Liquid,
    /// Sessions, Explorer and Changes rise from the line under them.
    sidebar_tab_liquid: crate::liquid::Liquid,
    /// Each project's working grid in the rail, by project path.
    project_marks: HashMap<String, Entity<vorkspace_ui::session_mark::ProjectMark>>,

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
    session_details: HashMap<i64, vorkspace_core::store::SessionMetadata>,
    sidebar_hidden_sessions: HashSet<i64>,
    worktree_returns: HashMap<(String, String), i64>,
    tab_drag: Option<TabDrag>,
    /// Tabs sliding between slots: neighbours making room for a drag and,
    /// after a drop, every tab gliding from where it was drawn into place.
    tab_shifts: HashMap<TitleTab, TabShift>,
    pane_ticks: HashMap<i64, PaneTick>,
    skip_tab_click: bool,
    error: Option<String>,
    tab_scroll: ScrollHandle,
    tab_scroll_task: Option<gpui::Task<()>>,
    resize: Option<(Pixels, f32)>,
    pub(crate) git: Entity<crate::git_status::ProjectGit>,
    pub(crate) right_panel: Option<Entity<RightPanel>>,
    right_resize: Option<(Pixels, f32)>,
    right_slide: Option<PaneSlide>,
    reviews: HashMap<i64, Entity<crate::git_tools::review::ReviewPane>>,
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
    right_drawn: f32,
    /// Classic: the corner tab opening out and the one folding away.
    corner_reveal: Option<crate::project_tabs::TabReveal>,
    dock_shifts: HashMap<gpui::EntityId, TabShift>,
    dock_skip_click: bool,
    dock_tab_scroll: ScrollHandle,
    move_terminal_anchor: AnchorSlot,
    /// The sidebar collapsed itself to make room; it returns with the room.
    sidebar_auto_hidden: bool,
    viewport_width: f32,
    viewport_height: f32,
    observations: Vec<Subscription>,
    generation: Arc<AtomicU64>,
    latest_state: Arc<Mutex<String>>,
    pub(crate) render_passes: usize,
}
impl Shell {
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
        Self::new_with_metadata(store, summaries, cwd, saved, metadata, cx)
    }
    pub(crate) fn new_with_metadata(
        store: Database,
        summaries: Vec<Session>,
        cwd: String,
        saved: Option<String>,
        sidebar_metadata: vorkspace_core::store::SessionSidebarMetadata,
        cx: &mut Context<Self>,
    ) -> Self {
        crate::accounts::install(store.clone(), cx);
        let remote_dir = locked(&store)
            .ok()
            .and_then(|store| store.data_dir().map(std::path::Path::to_path_buf))
            .unwrap_or_else(|| {
                std::env::temp_dir().join(format!("vorkspace-remote-{}", std::process::id()))
            });
        let remote = Arc::new(vorkspace_core::remote::Connections::new(remote_dir));
        vorkspace_core::remote::set_default_connections(remote.clone());
        let quit_remote = remote.clone();
        cx.on_app_quit(move |_, cx| {
            let remote = quit_remote.clone();
            cx.background_executor().spawn(async move {
                remote.shutdown();
            })
        })
        .detach();
        let remote_clock = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(3))
                    .await;
                if this
                    .update(cx, |this, cx| this.refresh_remote_projects(cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let metadata: Vec<_> = summaries.iter().map(|s| (s.id, s.cwd.clone())).collect();
        let workspace = Workspace::restore(saved.as_deref(), &metadata, &cwd);
        vorkspace_core::cli::initialize(&workspace.preferences.binary_paths);
        let latest_state = Arc::new(Mutex::new(serde_json::to_string(&workspace).unwrap()));
        let quit_state = latest_state.clone();
        let quit_store = store.clone();
        // This application-level hook survives the last window/entity being released.
        let app: &mut App = cx;
        app.on_app_quit(move |cx| {
            let state = quit_state.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let store = quit_store.clone();
            cx.background_executor().spawn(async move {
                let _ = locked(&store).and_then(|s| s.save_ui_state(&state));
            })
        })
        .detach();
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
                        this.poll_reminders(cx);
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
        let git = cx.new(|cx| crate::git_status::ProjectGit::new(git_project, cx));
        let git_observation = cx.observe(&git, |this, _, cx| {
            // The title bar's dot and the panel's tab label follow its stats.
            let _ = this;
            cx.notify();
        });
        let resources = cx.new(ResourceMonitor::new);
        let resource_observation = cx.observe(&resources, |_, _, cx| cx.notify());
        let resource_stops = cx.subscribe(&resources, |this, _, StopSession(id), cx| {
            if let Some(session) = this.sessions.get(id) {
                session.update(cx, |session, cx| session.stop_if_busy(cx));
            } else if let Some(m) = this.bots.clone() {
                m.update(cx, |m, cx| m.stop_session(*id, cx));
            }
        });
        let worktree_focus = workspace.worktree_focus.clone();
        let session_worktrees = sidebar_metadata.worktrees;
        let notification_state = notifications::State::new(&workspace, cx);
        let owner = cx.weak_entity();
        let live_agents = cx.new(|cx| live_agents::LiveAgents::new(owner, cx));
        let mut this = Self {
            #[cfg(not(test))]
            status_menu: None,
            bots: None,
            vox: None,
            quick_open: None,
            image_lightbox: None,
            app_bridge: None,
            orchestrations: HashMap::new(),
            orchestration_summaries: sidebar_metadata.orchestrations,
            started_by: sidebar_metadata.started_by,
            rails: Default::default(),
            usage_footer: cx.new(|cx| UsageFooter::new(active_kind, cx)),
            resources,
            sidebar_frame,
            project_rail_frame,
            project_rail_drag: None,
            sidebar_search,
            sidebar_list: sidebar_list::SidebarList::new(cx),
            sidebar_slide: None,
            sidebar_fraction: if workspace.sidebar_visible { 1. } else { 0. },
            sidebar_peek: None,
            peek_animating: false,
            peek_close: None,
            peek_toggle_bounds: AnchorSlot::default(),
            fullscreen: false,
            tabs_frame,
            store,
            remote,
            remote_project_picker: None,
            remote_refreshing: false,
            remote_marks: HashMap::new(),
            _remote_clock: remote_clock,
            sessions: HashMap::new(),
            session_summaries: HashMap::new(),
            draft_sessions: sidebar_metadata.drafts,
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
            groups: sidebar_metadata.groups,
            reminders: sidebar_metadata.reminders,
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
            folder_bounds: HashMap::new(),
            sidebar_drop: None,
            folder_drag: None,
            skip_folder_click: false,
            filter_anchor: AnchorSlot::default(),
            folder_picker: None,
            pending_folder: None,
            _session_clock: session_clock,
            notes: None,
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
            saved_search: None,
            order: Vec::new(),
            workspace,
            external_editors: None,
            external_editor_scan: 0,
            deleting_projects: HashSet::new(),
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
            backgrounds: HashMap::new(),
            background_card: AnchorSlot::default(),
            liquid: Default::default(),
            tab_liquid: crate::liquid::Liquid::below(crate::panel_kit::stroke()),
            sidebar_tab_liquid: crate::liquid::Liquid::below(crate::panel_kit::stroke()),
            project_marks: HashMap::new(),

            project_anchor: AnchorSlot::default(),
            sidebar_options_anchor: AnchorSlot::default(),
            history: VisitHistory::default(),
            navigation_revision: 0,
            pending_project_blanks: HashSet::new(),
            pending_project_creations: HashMap::new(),
            new_tab_task: None,
            worktree_focus,
            session_worktrees,
            session_details: sidebar_metadata.details,
            sidebar_hidden_sessions: sidebar_metadata.sidebar_hidden,
            worktree_returns: HashMap::new(),
            tab_drag: None,
            tab_shifts: HashMap::new(),
            pane_ticks: HashMap::new(),
            skip_tab_click: false,
            error: None,
            tab_scroll: ScrollHandle::new(),
            tab_scroll_task: None,
            resize: None,
            git,
            right_panel: None,
            right_resize: None,
            right_slide: None,
            reviews: HashMap::new(),
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
            sidebar_auto_hidden: false,
            terminals: HashMap::new(),
            dock_slide: None,
            dock_resize: None,
            resize_settle: None,
            resize_pinned: false,
            dock_hover: None,
            dock_tab_motion: Default::default(),
            dock_drag: None,
            right_drawn: 0.,
            corner_reveal: None,
            dock_shifts: HashMap::new(),
            dock_skip_click: false,
            dock_tab_scroll: ScrollHandle::new(),
            move_terminal_anchor: AnchorSlot::default(),
            viewport_width: 1200.,
            viewport_height: 800.,
            observations: vec![
                git_observation,
                resource_observation,
                resource_stops,
                sidebar_search_events,
            ],
            generation: Arc::new(AtomicU64::new(0)),
            latest_state,
            render_passes: 0,
        };
        this.scan_external_editors(cx);
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
        let bots = cx.new(|cx| {
            crate::bots::Bots::new(
                this.store.clone(),
                bot_owner,
                this.workspace.preferences.clone(),
                bot_active,
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
            |_, _, request: &crate::bots::ShowDetails, cx| {
                let id = request.0.clone();
                cx.spawn(async move |this, cx| {
                    let _ = this.update_in(cx, |this, window, cx| {
                        if this
                            .bots
                            .as_ref()
                            .is_some_and(|m| m.read(cx).active.as_ref() == Some(&id))
                        {
                            this.set_right_panel_open(true, window, cx);
                        }
                    });
                })
                .detach();
            },
        ));
        let mut bot_chrome = bots.read(cx).overview_chrome();
        let mut bot_error = bots.read(cx).error.clone();
        this.observations
            .push(cx.observe(&bots, move |this, bots, cx| {
                let chrome = bots.read(cx).overview_chrome();
                if chrome != bot_chrome {
                    bot_chrome = chrome;
                    this.invalidate_chrome(cx);
                    cx.notify();
                }
                let error = &bots.read(cx).error;
                if *error != bot_error {
                    bot_error = error.clone();
                    cx.notify();
                }
            }));
        this.bots = Some(bots);
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
        if let Some(id) = this.workspace.active {
            this.ensure_session(this.workspace.session_for_pane(id), cx);
            this.record_worktree_session(id, cx);
        }
        if let Some(id) = this.workspace.active {
            this.history.record(id);
        }
        this.reveal_active_tab(cx);
        this.update_chat_visibility(cx);
        this.update_usage_provider(cx);
        this.apply_preferences(cx);
        this.poll_reminders(cx);
        this.initialize_automations(cx);
        cx.spawn(async move |this, cx| {
            let _ = this.update_in(cx, |this, window, cx| {
                if this.navigation_revision == 0 && !this.restore_scope_tab(window, cx) {
                    this.ensure_project_session(window, cx);
                }
                this.persist(cx);
            });
        })
        .detach();
        this
    }
    fn update_chat_visibility(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = &self.active_terminal_tab
            && let Some(terminal) = self.terminal_tabs.get(id)
        {
            let visible = self.settings.is_none() && self.app_search.is_none();
            terminal.update(cx, |terminal, _| terminal.visible = visible);
        }
        for (provider, page) in &self.connector_pages {
            let active = self.active_app_view == Some(app_views::AppView::Connector(*provider))
                && self.settings.is_none()
                && self.app_search.is_none();
            page.update(cx, |page, cx| page.set_active(active, cx));
        }
        // Every pane of the active tab is on screen, not just the focused one.
        let shown: HashSet<i64> = match self.workspace.active_tab() {
            Some(tab)
                if !self.app_view_open()
                    && self.settings.is_none()
                    && self.app_search.is_none()
                    && !self.bot_active(cx) =>
            {
                self.workspace.layout(tab).leaf_ids().into_iter().collect()
            }
            _ => HashSet::new(),
        };
        // Hidden tabs keep their prepared image and any in-flight load. Only
        // closing a tab/pane should discard its background.
        let open: HashSet<i64> = self.workspace.open_sessions().into_iter().collect();
        self.backgrounds.retain(|id, _| open.contains(id));
        for &id in &shown {
            self.ensure_session(id, cx);
            self.history_clock += 1;
            self.history_recency.insert(id, self.history_clock);
        }
        for (&id, view) in &self.sessions {
            view.update(cx, |v, cx| {
                if shown.contains(&id) {
                    v.load(cx);
                }
                v.transcript
                    .update(cx, |t, cx| t.visible(shown.contains(&id), cx))
            });
        }
        self.trim_session_histories(&shown, cx);
    }
    fn add(&mut self, summary: Session, cx: &mut Context<Self>) {
        self.workspace.restore_project(&summary.cwd);
        self.add_loaded(summary, cx);
    }
    /// Loading history must not turn every session directory into a rail project.
    fn add_loaded(&mut self, summary: Session, cx: &mut Context<Self>) {
        let id = summary.id;
        if self.sessions.contains_key(&id) {
            return;
        }
        self.session_summaries.insert(id, summary.clone());
        self.session_marks
            .insert(id, cx.new(|_| SessionMark::new(summary.provider.clone())));
        self.tab_marks
            .insert(id, cx.new(|_| SessionMark::tab(summary.provider.clone())));
        let appearance = self.appearance(&summary.cwd);
        let worktree = self.session_worktrees.get(&id).cloned().flatten();
        let existing = self
            .bot_conversations
            .borrow()
            .get(&id)
            .and_then(|view| view.upgrade());
        let view = existing
            .unwrap_or_else(|| cx.new(|cx| Conversation::new(summary, self.store.clone(), cx)));
        view.update(cx, |view, cx| {
            // Events may arrive before the first asynchronous agent load. Seed
            // the indexed working directory so those events cannot erase it.
            view.restore_worktree(worktree, None, cx);
            view.draft_hint = self.draft_sessions.contains(&id);
            if let Some(bridge) = &self.app_bridge {
                view.set_app_environment(bridge.environment(id));
            }
            view.minions = Some(self.minion_registry.clone());
            view.team_histories = Some(self.team_histories.clone());
            view.refresh_team_history(cx);
            view.set_project_appearance(appearance, cx);
            view.transcript.update(cx, |transcript, cx| {
                transcript.apply_preferences(&self.workspace.preferences, cx)
            });
        });
        self.observations
            .push(cx.subscribe(&view, move |this, _, _: &SessionChanged, cx| {
                let Some(view) = this.sessions.get(&id) else {
                    return;
                };
                this.session_worktrees
                    .insert(id, view.read(cx).settings.worktree_cwd.clone());
                // A finished turn may have edited files or moved to a worktree.
                if this.workspace.active == Some(id) {
                    this.sync_git(cx);
                    this.git.update(cx, |git, cx| git.reload(cx));
                }
                this.sync_search_session(id, cx);
                this.orchestration_session_changed(id, cx);
                this.automation_session_changed(id, cx);
                this.sync_review_locks(cx);
                this.refresh_session_mark(id, cx);
                this.notification_session_changed(id, cx);
                this.update_usage_provider(cx);
                this.update_chat_visibility(cx);
                this.invalidate_chrome(cx);
                cx.notify();
            }));
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
                }
            }));
        self.observe_orchestration(&view, id, cx);
        self.observe_session_folders(&view, id, cx);
        self.sessions.insert(id, view);
        if !self.order.contains(&id) {
            self.order.push(id);
        }
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
        let id = self.workspace.session_for_pane(id);
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
        if !self.archiving.insert(id) {
            return;
        }
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
            let mut summary = locked(&store)?.create(&transfer.target.provider, &transfer.cwd)?;
            summary.title = if transfer.kind == TransferKind::SecondOpinion {
                "Second opinion".into()
            } else {
                transfer.title
            };
            locked(&store)?.title(summary.id, &summary.title)?;
            locked(&store)?.configure_agent(summary.id, &summary.provider, &settings)?;
            Ok::<_, String>((summary, settings, prompt))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok((summary, settings, prompt)) => {
                    let id = summary.id;
                    this.add(summary, cx);
                    this.sessions[&id]
                        .update(cx, |view, cx| view.install_transfer(settings, prompt, cx));
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
        self.sidebar_list.invalidate_structure();
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
            .and_then(|id| self.sessions.get(&self.workspace.session_for_pane(id)))
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
        if !self.unsaved_files(cx) {
            crate::quit(cx);
            return;
        }
        if self.quit_prompted {
            return;
        }
        self.quit_prompted = true;
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            "Quit with unsaved file edits?",
            Some("Save your files before quitting to keep your changes."),
            &["Quit Without Saving", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await;
            let _ = this.update(cx, |this, cx| {
                this.quit_prompted = false;
                if matches!(answer, Ok(0)) {
                    crate::quit(cx);
                }
            });
        })
        .detach();
    }
    fn request_restart(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.unsaved_files(cx) {
            crate::restart(cx);
            return;
        }
        if self.quit_prompted {
            return;
        }
        self.quit_prompted = true;
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            "Restart with unsaved file edits?",
            Some("Save your files before restarting to keep your changes."),
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
    fn add_to_chat(&mut self, id: i64, text: &str, cx: &mut Context<Self>) {
        let Some(view) = self.ensure_session(id, cx) else {
            return;
        };
        let input = view.read(cx).input.clone();
        if input.read(cx).is_composing() {
            return;
        }
        input.update(cx, |input, cx| {
            let draft = input.content.to_string();
            let separator = if draft.is_empty() || draft.ends_with("\n\n") {
                ""
            } else if draft.ends_with('\n') {
                "\n"
            } else {
                "\n\n"
            };
            input.set_text(format!("{draft}{separator}{}\n\n", text.trim()), cx);
        });
        self.chat_hidden_off(id, cx);
        self.pending_composer_focus = Some(id);
        cx.notify();
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
    /// A panel or pane that would squeeze a chat below a readable width
    /// collapses the sidebar as it opens. A sidebar collapsed that way comes
    /// back once there is room again; one the user hid stays hidden.
    fn fit_sidebar(&mut self, cx: &mut Context<Self>) {
        // These surfaces replace project tools rather than competing with them
        // for space. Preserve the session sidebar's own visibility preference.
        if self.bot_active(cx)
            || self.app_view_open()
            || self.app_search.is_some()
            || self.settings.is_some()
        {
            return;
        }
        if self.workspace.sidebar_visible {
            if self.narrowest_chat(true, cx) < MIN_CHAT_WIDTH {
                self.set_sidebar_visible(false, cx);
                self.sidebar_auto_hidden = true;
            }
        } else if self.sidebar_auto_hidden && self.narrowest_chat(true, cx) >= MIN_CHAT_WIDTH {
            self.set_sidebar_visible(true, cx);
            self.sidebar_auto_hidden = false;
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
        self.update_chat_visibility(cx);
        self.update_usage_provider(cx);
        self.invalidate_chrome(cx);
        let state = serde_json::to_string(&self.workspace).unwrap();
        *self.latest_state.lock().unwrap_or_else(|e| e.into_inner()) = state.clone();
        let revision = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let generation = self.generation.clone();
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            let store = locked(&store)?;
            if generation.load(Ordering::SeqCst) == revision {
                store.save_ui_state(&state)?;
            }
            Ok::<_, String>(())
        });
        cx.spawn(async move |this, cx| {
            if let Err(error) = work.await {
                let _ = this.update(cx, |this, cx| {
                    this.error = Some(format!("Could not save workspace: {error}"));
                    cx.notify();
                });
            }
        })
        .detach();
    }
    fn update_usage_provider(&self, cx: &mut Context<Self>) {
        let view = self
            .bots
            .as_ref()
            .filter(|_| self.bot_active(cx))
            .and_then(|m| m.read(cx).chat())
            .or_else(|| {
                self.workspace.active.and_then(|id| {
                    self.sessions
                        .get(&self.workspace.session_for_pane(id))
                        .cloned()
                })
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
        if self.focus_app_view(window, cx) {
            return;
        }
        if let Some(bots) = self
            .bots
            .clone()
            .filter(|m| m.read(cx).active.is_some() || m.read(cx).classic_channels_open())
        {
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
        if self.workspace.review_owners.contains_key(&id) {
            self.open_file_pane(id, window, cx);
            return;
        }
        if !self.session_summaries.contains_key(&id) || self.deleting.contains(&id) {
            return;
        }
        self.navigation_revision += 1;
        self.replace_blank_pane(id, cx);
        self.activate_session(id, window, cx);
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
        if self.workspace.review_owners.contains_key(&id) {
            self.open_file_pane(id, window, cx);
            return;
        }
        if let Some(bots) = &self.bots {
            bots.update(cx, |m, cx| m.close(cx));
        }
        self.leave_app_views(cx);
        self.leave_app_search(cx);
        self.close_preview(cx);
        self.dismiss_due_reminder(id, cx);
        let Some(view) = self.ensure_session(id, cx) else {
            return;
        };
        self.history.record(id);
        self.workspace.open(id);
        self.workspace.project = view.read(cx).summary.cwd.clone();
        self.workspace
            .project_focus
            .insert(self.workspace.project.clone(), id);
        self.workspace.collapsed.remove(&self.workspace.project);
        view.update(cx, |v, cx| v.load(cx));
        self.record_worktree_session(id, cx);
        self.remember_title_tab(TitleTab::Session(self.workspace.tab_of(id).unwrap()));
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
            .map(TitleTab::Session)
            .collect::<Vec<_>>();
        let preferred = preferred
            .and_then(|id| self.workspace.tab_of(id))
            .map(TitleTab::Session);
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
        if let Some(settings) = &self.settings {
            window.focus(&settings.focus_handle(cx), cx);
            return;
        }
        window.focus(&self.focus, cx);
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
        self.sidebar_auto_hidden = false;
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
        self.drop_peek();
        self.workspace.sidebar_visible = visible;
        self.sync_right_panel(cx);
    }
    fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_app_search(window, cx);
    }

    fn select_project(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .deleting_projects
            .contains(&vorkspace_core::files::path_key(&path))
        {
            self.error = Some("This project is being deleted.".into());
            cx.notify();
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
        self.query.clear();
        self.sidebar_search
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        self.workspace.collapsed.remove(&path);
        // Choosing a project restores its last conversation or terminal.
        // Global Bot/app surfaces still require explicit navigation.
        if matches!(
            self.workspace.scope_focus.get(&self.tab_scope()),
            Some(TitleTab::Session(_) | TitleTab::Terminal { .. })
        ) && self.restore_scope_tab(window, cx)
        {
            return;
        }
        self.workspace.active = self
            .workspace
            .project_focus
            .get(&path)
            .copied()
            .filter(|id| {
                self.workspace.tab_of(*id).is_some()
                    && self
                        .session_summaries
                        .contains_key(&self.workspace.session_for_pane(*id))
                    && self
                        .session_metadata(self.workspace.session_for_pane(*id), cx)
                        .summary
                        .cwd
                        == path
                    && self.in_worktree_scope(*id, cx)
            })
            .or_else(|| {
                self.workspace
                    .tabs
                    .iter()
                    .rev()
                    .find(|id| {
                        self.session_summaries.contains_key(
                            &self
                                .workspace
                                .session_for_pane(self.workspace.tab_focus(**id)),
                        ) && self
                            .session_metadata(
                                self.workspace
                                    .session_for_pane(self.workspace.tab_focus(**id)),
                                cx,
                            )
                            .summary
                            .cwd
                            == path
                            && self.in_worktree_scope(self.workspace.tab_focus(**id), cx)
                    })
                    .map(|tab| self.workspace.tab_focus(*tab))
            });
        if let Some(id) = self.workspace.active {
            self.activate_session(id, window, cx);
        } else {
            self.ensure_project_session(window, cx);
        }
        self.focus_active(window, cx);
        self.persist(cx);
        cx.notify();
    }
    fn ensure_project_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.active.is_some()
            || self.bot_active(cx)
            || self.active_terminal_tab.is_some()
        {
            return;
        }
        if let Some(id) = self.order.iter().copied().find(|id| {
            let s = self.session_metadata(*id, cx);
            s.summary.cwd == self.workspace.project
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
                        this.workspace.restore_project(&path);
                        this.select_project(path, window, cx);
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
    fn create(&mut self, kind: Kind, cx: &mut Context<Self>) {
        self.create_session(kind, false, cx);
    }
    fn create_session(&mut self, kind: Kind, only_if_empty: bool, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let cwd = self.workspace.project.clone();
        let focus = self.worktree_focus.get(&cwd).cloned();
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
        let work = cx.background_executor().spawn({
            let cwd = cwd.clone();
            let worktree_cwd = focus.as_ref().map(|f| f.path.clone());
            async move {
                let store = locked(&store)?;
                let summary = store.create(kind.id(), &cwd)?;
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
                Ok::<_, String>((summary, settings))
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
                    Ok((summary, settings)) => {
                        let id = summary.id;
                        this.add(summary, cx);
                        if let Some(view) = this.sessions.get(&id) {
                            view.update(cx, |view, cx| view.seed_new_settings(settings, cx));
                        }
                        this.seed_worktree(id, focus.as_ref(), cx);
                        // A pending blank belongs to the copy that requested it;
                        // navigation to another copy must never redirect that request.
                        if this.workspace.project != cwd
                            || this.worktree_focus.get(&cwd).map(|f| &f.path)
                                != focus.as_ref().map(|f| &f.path)
                            || if only_if_empty {
                                this.workspace.active.is_some()
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
        if let Some(id) = self.pending_composer_focus.take()
            && self
                .workspace
                .active
                .map(|pane| self.workspace.session_for_pane(pane))
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
            m.read(cx).hide_html(cx);
            m.update(cx, |m, _| m.bind_window(window));
        }
        #[cfg(not(test))]
        self.sync_status_menu(window, cx);
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
        if configure(p, system_light, self.workspace.classic_layout) {
            cx.refresh_windows();
        }
        let glass = window_glass(p, system_light, self.workspace.classic_layout);
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
        if self.settings.is_some()
            || self.app_view_open()
            || self.app_search.is_some()
            || self.bot_active(cx)
        {
            self.sidebar_fraction = 0.;
        }
        let sidebar_width = self.workspace.right_panel_width * self.sidebar_fraction;
        if !self.window_observed {
            self.window_observed = true;
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
                        let sessions: Vec<_> = this.sessions.values().cloned().collect();
                        for view in sessions {
                            view.update(cx, |v, cx| v.refresh_if_stale(cx));
                        }
                    }
                    this.refresh_notification_permission();
                    cx.notify();
                }));
            self.start_native_notifications(cx);
            let shell = cx.entity().downgrade();
            window.on_window_should_close(cx, move |window, cx| {
                shell
                    .update(cx, |shell, cx| {
                        #[cfg(not(test))]
                        if shell.workspace.preferences.close_to_tray
                            && shell.status_menu.is_some()
                            && !shell.unsaved_files(cx)
                        {
                            vorkspace_ui::traffic_lights::hide(window);
                            return false;
                        }
                        if shell.unsaved_files(cx) {
                            shell.request_quit(window, cx);
                            false
                        } else if let Some(voice) =
                            shell.vox.as_ref().filter(|v| v.read(cx).running()).cloned()
                        {
                            voice.update(cx, |v, cx| v.stop(cx));
                            let handle = window.window_handle();
                            cx.spawn(async move |_, cx| {
                                for _ in 0..80 {
                                    if !voice.read_with(cx, |v, _| v.running()) {
                                        break;
                                    }
                                    cx.background_executor()
                                        .timer(std::time::Duration::from_millis(100))
                                        .await;
                                }
                                let _ = handle.update(cx, |_, window, _| window.remove_window());
                            })
                            .detach();
                            false
                        } else {
                            if let Some(voice) = &shell.vox {
                                voice.update(cx, |voice, cx| voice.stop(cx));
                            }
                            true
                        }
                    })
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
        let full = self.workspace.right_panel_width;
        let right_width = match &self.right_slide {
            Some(slide) => {
                let (width, done) = slide.at(vorkspace_ui::clock::Instant::now());
                if done {
                    self.right_slide = None;
                    self.sync_right_panel(cx);
                    if self.workspace.right_panel_open
                        && !self.bot_active(cx)
                        && let Some(panel) = self.right_panel.clone()
                    {
                        panel.update(cx, |p, cx| p.finish_opening(window, cx));
                    }
                } else {
                    window.request_animation_frame();
                }
                width
            }
            None => full,
        };
        let right_shown = self.right_panel_shown(cx);
        self.right_drawn = if right_shown { right_width } else { 0. };
        let poll_git = self.window_active && !self.bot_active(cx);
        self.git.update(cx, |git, cx| {
            git.set_polling(poll_git && git.available(), cx)
        });
        if let Some(panel) = &self.right_panel {
            let tab_width = self.project_tabs_width(cx);
            panel.update(cx, |p, cx| {
                p.set_top_inset(0., cx);
                p.set_tab_width(tab_width, cx);
                p.set_show_excluded(self.workspace.preferences.show_excluded_files, cx);
            });
        }
        if let Some(bots) = &self.bots {
            let details = bots.read(cx).details.clone();
            let inset = if self.workspace.classic_layout {
                TITLE_BAR_HEIGHT
            } else {
                0.
            };
            details.update(cx, |view, cx| view.set_top_inset(inset, cx));
        }
        self.sync_right_panel(cx);
        self.sync_notices(window, cx);
        div()
            .id("vorkspace-workspace")
            .role(Role::Application)
            .aria_label("vorkspace Native")
            .key_context("Workspace")
            .track_focus(&self.focus)
            .on_action(
                cx.listener(|this, open: &crate::image_lightbox::Open, window, cx| {
                    if let Some((old, _)) = &this.image_lightbox {
                        old.update(cx, |view, cx| view.reject_replacement(cx));
                        cx.stop_propagation();
                        return;
                    }
                    let owner = open.draft_session.and_then(|id| {
                        this.sessions.get(&id).cloned().or_else(|| {
                            this.bots
                                .as_ref()
                                .and_then(|bots| bots.read(cx).conversation_for_session(id, cx))
                        })
                    });
                    let owner = owner
                        .as_ref()
                        .map(crate::conversation::composer_features::annotation_owner);
                    let view = cx.new(|cx| {
                        crate::image_lightbox::ImageLightbox::new(open.clone(), owner, window, cx)
                    });
                    let closed =
                        cx.subscribe(&view, |this, _, _: &crate::image_lightbox::Closed, cx| {
                            this.image_lightbox = None;
                            cx.notify();
                        });
                    this.image_lightbox = Some((view, closed));
                    cx.notify();
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
            .on_action(cx.listener(
                |this, event: &crate::file_tools::file_editor::OpenPath, window, cx| {
                    let cwd = this.git.read(cx).project().to_owned();
                    let (path, location) =
                        crate::file_tools::file_editor::file_location(&event.0, &cwd);
                    this.open_review(
                        crate::git_tools::review::ReviewTab::File { cwd, path },
                        window,
                        cx,
                    );
                    // Bots render above the session pane tree. Give its file
                    // an independent visible tab using the normal navigation
                    // path, which retains the Bot tab and live conversation.
                    if this.bot_active(cx)
                        && let Some(owner) = this.workspace.active
                        && let Some(pane) = this.independent_file_pane(owner, cx)
                    {
                        this.open_file_pane(pane, window, cx);
                    }
                    if let Some((line, column)) = location
                        && let Some(pane) = this.active_review(cx)
                    {
                        pane.update(cx, |pane, cx| pane.navigate_file(line, column, cx));
                    }
                },
            ))
            .on_action(
                cx.listener(|this, _: &crate::Quit, window, cx| this.request_quit(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &crate::Restart, window, cx| {
                    this.request_restart(window, cx)
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
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
                this.open_settings(crate::settings::Section::General, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &crate::clipboard::Toggle, window, cx| {
                    this.status_menu_action(
                        vorkspace_macos::status_menu::Action::Clipboard {
                            status_click: false,
                        },
                        window,
                        cx,
                    )
                }),
            )
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
                if this.settings.is_none()
                    && this.active_terminal_tab.is_some()
                    && let Some(tab) = this.active_title_tab(cx)
                {
                    this.close_title_tabs(&[tab], None, window, cx);
                    return;
                }
                if this.active_app_view.is_some() && this.settings.is_none() {
                    this.close_app_view(window, cx);
                    return;
                }
                if this.app_search.is_some() {
                    this.close_search_tab(window, cx);
                    return;
                }
                if this.bot_active(cx) {
                    this.close_bot_page(window, cx);
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
                    if this.peek_open() && this.menu.is_none() {
                        this.set_peek(false, cx);
                        return;
                    }
                    if this.menu.is_none()
                        && this.right_panel_shown(cx)
                        && !this.bot_active(cx)
                        && let Some(panel) = &this.right_panel
                        && panel.read(cx).tab() == crate::panel_kit::RightTab::Files
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
                    this.folder_drag = None;
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
            .on_action(cx.listener(|this, _: &ToggleRightPanel, window, cx| {
                this.toggle_right_panel(window, cx)
            }))
            .on_action(cx.listener(|this, _: &SearchFiles, window, cx| {
                if this.app_view_open() || this.settings.is_some() || this.bot_active(cx) {
                    return;
                }
                this.close_app_search(window, cx);
                this.ensure_right_panel(window, cx);
                this.set_right_panel_open(true, window, cx);
                if let Some(panel) = &this.right_panel {
                    let explorer = panel.read(cx).explorer.clone();
                    panel.update(cx, |panel, cx| {
                        panel.select(crate::panel_kit::RightTab::Files, cx)
                    });
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
            .on_action(|_: &FocusNext, window, cx| window.focus_next(cx))
            .on_action(|_: &FocusPrevious, window, cx| window.focus_prev(cx))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if let Some(bots) = &this.bots {
                    bots.update(cx, |m, cx| m.move_rail_drag(event, cx));
                }
                this.move_tab_drag(event, cx);
                this.move_dock_drag(event, cx);
                this.move_pane_drag(event, cx);
                this.move_folder_drag(event, cx);
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
                        // Its separator faces the pane, on either side.
                        let width = if !this.workspace.preferences.sidebar_right {
                            width + delta
                        } else {
                            width - delta
                        };
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
                    this.finish_folder_drag(cx);
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
            .child(self.right_panel_resize_watch(cx))
            .children(self.layout_body(sidebar_width, right_shown, right_width, full, window, cx))
            .children(self.peek_watch(window, cx))
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
                        p.align(Align::Start)
                    })
            }))
            .children(self.drag_ghost(cx))
            .children(self.background_modal(window, cx))
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
            .children(self.folder_picker_popover(window, cx))
            .children(crate::accounts::get(cx))
            .children(
                self.image_lightbox
                    .as_ref()
                    .map(|(view, _)| crate::image_lightbox::overlay(view)),
            )
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
            assert!(visual.debug_bounds("sidebar-resize").is_none(), "{label}");
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
                    visual.debug_bounds("sidebar-content").is_none()
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
