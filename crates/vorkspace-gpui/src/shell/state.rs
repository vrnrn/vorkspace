//! Screen state owned by Shell. Member order preserves resource release order.
use super::*;

pub(super) struct SidebarState {
    /// Each lead's last orchestration, whose agents hang under it in the sidebar.
    pub(super) orchestration_summaries: HashMap<i64, vorkspace_core::orchestration::Summary>,
    /// Sessions an /operator thread started, by the session that started them.
    pub(super) started_by: HashMap<i64, i64>,
    pub(super) slide: Option<PaneSlide>,
    pub(super) fraction: f32,
    pub(super) content_width: f32,
    pub(super) live_agents: Entity<live_agents::LiveAgents>,
    pub(super) groups: HashMap<String, Arc<session_groups::FolderIndex>>,
    pub(super) renaming: Option<session_groups::Renaming>,
    pub(super) link_dialog: Option<session_groups::LinkDialog>,
    pub(super) group_writes: Option<gpui::Task<()>>,
    pub(super) selected_sessions: HashSet<i64>,
    pub(super) selection_anchor: Option<i64>,
    pub(super) context_selection: bool,
    pub(super) target: Option<session_groups::RenameTarget>,
    pub(super) session_row_bounds: HashMap<i64, AnchorSlot>,
    pub(super) filter_anchor: AnchorSlot,
    pub(super) query: String,
    pub(super) hover: Option<SidebarNavigation>,
    /// Each project's working grid in the rail, by project path.
    pub(super) project_anchor: AnchorSlot,
    pub(super) options_anchor: AnchorSlot,
    pub(super) resize: Option<(Pixels, f32)>,
    pub(super) skip_session_click: bool,
}

pub(super) struct PageTabs {
    pub(super) notes: Option<Entity<Notes>>,
    pub(super) codebase_tabs: HashMap<i64, Entity<crate::codebase::Page>>,
    pub(super) note_tabs: HashMap<i64, Entity<Notes>>,
    pub(super) item_tabs: HashMap<i64, Entity<crate::inbox::Inbox>>,
    pub(super) bot_document_tabs: HashMap<i64, Entity<crate::bots::BotDocumentPane>>,
    pub(super) bot_pane_views: HashMap<i64, Entity<app_panes::BotPaneView>>,
    pub(super) todos_page: Option<Entity<crate::todos::TodosPage>>,
}

pub(super) struct AppPages {
    pub(super) inbox: Option<Entity<crate::inbox::Inbox>>,
    pub(super) connector_pages: HashMap<crate::workspace::Connector, Entity<crate::inbox::Inbox>>,
    pub(super) automations: Option<Entity<crate::automations::Automations>>,
    pub(super) active_app_view: Option<app_views::AppView>,
    pub(super) active_terminal_tab: Option<u64>,
    /// Title-tab shells have exclusive ownership, separate from dock shells.
    pub(super) terminal_tabs: HashMap<u64, Entity<Terminal>>,
    pub(super) view_events: Vec<Subscription>,
    pub(super) automation_state: automation_runs::State,
    pub(super) settings: Option<Entity<crate::settings::SettingsView>>,
    pub(super) settings_events: Option<Subscription>,
    pub(super) search: Option<search::Open>,
    pub(super) parked_search: Option<search::Open>,
    pub(super) saved_search: Option<(String, vorkspace_core::search::Scope)>,
}

pub(crate) struct WindowState {
    /// useTrafficLightsVisible: fullscreen hides the native buttons, so the
    /// title bar takes their room back.
    pub(super) fullscreen: bool,
    pub(super) observed: bool,
    pub(super) shortcuts: Option<shortcuts::Shortcuts>,
    pub(super) active: bool,
    pub(super) glass: Option<bool>,
    pub(super) blur: Option<u32>,
    pub(super) quit_prompted: bool,
    pub(super) layout: u64,
    pub(super) closing: bool,
    pub(super) native_window: Option<gpui::WindowHandle<Shell>>,
    pub(super) native_title: String,
    /// The sidebar collapsed itself to make room; it returns with the room.
    pub(super) viewport_width: f32,
    pub(super) viewport_height: f32,
    pub(crate) render_passes: usize,
}

pub(crate) struct ProjectSettingsState {
    pub(super) background_dialog: Option<project_settings::OpenBackground>,
    /// Project settings: one project's appearance, background, Codebase
    /// Memory and notifications, in the same dialog frame as a Bot's.
    pub(crate) vorkspace_dialog: Option<vorkspace_dialog::VorkspaceDialog>,
}

pub(super) struct ProjectRailState {
    pub(super) drag: Option<(Pixels, f32)>,
    pub(super) sessions_expanded: bool,
    pub(super) list: gpui::ListState,
    pub(super) list_ids: std::rc::Rc<Vec<i64>>,
    /// The rail's title search field is showing.
    pub(super) search_open: bool,
    pub(super) vorkspace_anchor: AnchorSlot,
}

pub(super) struct TabStripState {
    pub(super) hover: Option<i64>,
    /// Tab animations: opening and closing title tabs, with each tab's last label.
    pub(super) motion: vorkspace_ui::tab_motion::TabMotion<TitleTab, ()>,
    /// Tab animations: the selected tab before the last switch, the one now
    /// selected and when it changed, so one eases down as the other widens.
    pub(super) selection: (
        Option<TitleTab>,
        Option<TitleTab>,
        vorkspace_ui::clock::Instant,
    ),
    pub(super) pending_project_blanks: HashSet<String>,
    pub(super) new_tab_task: Option<gpui::Task<()>>,
    pub(super) drag: Option<TabDrag>,
    /// Tabs sliding between slots: neighbours making room for a drag and,
    /// after a drop, every tab gliding from where it was drawn into place.
    pub(super) shifts: HashMap<TitleTab, TabShift>,
    pub(super) pane_ticks: HashMap<i64, PaneTick>,
    pub(super) skip_tab_click: bool,
    pub(super) scroll: ScrollHandle,
    pub(super) switcher_anchor: AnchorSlot,
    pub(super) scroll_task: Option<gpui::Task<()>>,
}

pub(super) struct ReviewState {
    pub(super) reviews: HashMap<i64, Entity<crate::git_tools::review::ReviewPane>>,
    pub(super) cache: crate::git_tools::review::ReviewCache,
    /// A review sliding in or out: its session and pane.
    pub(super) slide: Option<(i64, PaneSlide, Entity<crate::git_tools::review::ReviewPane>)>,
    /// A session review card's Review, opened beside its pane next frame.
    pub(super) pending_review: Option<(i64, crate::git_tools::review::ReviewTab)>,
    pub(super) focused: bool,
    pub(super) drag: bool,
}

pub(super) struct PaneState {
    pub(super) preparing_panes: bool,
    pub(super) split_bounds: HashMap<i64, AnchorSlot>,
    pub(super) bounds: AnchorSlot,
    pub(super) viewport: AnchorSlot,
    pub(super) scroll: ScrollHandle,
    pub(super) scroll_focus: Option<(i64, i64, u32, u32)>,
    pub(super) drag: Option<PaneDrag>,
    pub(super) sash: Option<SashDrag>,
    pub(super) motion: Option<PaneMotion>,
}

pub(super) struct TerminalDockState {
    /// Live terminals of each project's dock, in the dock's tab order.
    pub(super) terminals: HashMap<String, Vec<Entity<Terminal>>>,
    /// A project's dock sliding open (toward 1) or shut (toward 0).
    pub(super) slide: Option<(String, PaneSlide)>,
    pub(super) resize: Option<(f32, f32)>,
    pub(super) hover: Option<usize>,
    /// Tab animations for the dock's terminals, with each tab's last title.
    pub(super) tab_motion: vorkspace_ui::tab_motion::TabMotion<gpui::EntityId, SharedString>,
    pub(super) drag: Option<terminal_dock::DockTabDrag>,
    /// The right panel's drawn width this frame, 0 while it is hidden.
    /// Classic: the corner tab opening out and the one folding away.
    pub(super) shifts: HashMap<gpui::EntityId, TabShift>,
    pub(super) skip_click: bool,
    pub(super) tab_scroll: ScrollHandle,
    pub(super) move_terminal_anchor: AnchorSlot,
}

pub(super) struct ExternalEditorState {
    pub(super) editors: Option<Vec<vorkspace_core::external_editor::ExternalEditor>>,
    pub(super) scan: u64,
}

pub(super) struct SidebarPreviewState {
    pub(super) session_hover: Option<i64>,
    pub(super) session_preview: Option<SessionPreview>,
    pub(super) close: Option<gpui::Task<()>>,
    pub(super) open: Option<gpui::Task<()>>,
    pub(super) last_scroll: Option<vorkspace_ui::clock::Instant>,
}
