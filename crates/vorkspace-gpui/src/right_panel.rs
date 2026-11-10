//! Explorer, Changes and History share one sidebar focus and visibility owner.
use crate::{
    file_tools::explorer::Explorer,
    git_status::ProjectGit,
    git_tools::{changes::Changes, history::History},
    panel_kit::RightTab,
};
use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, Role, StyleRefinement, Window, div,
    prelude::*,
};

pub(crate) const WIDTH_MIN: f32 = 220.;
pub(crate) const WIDTH_MAX: f32 = 640.;
pub(crate) const WIDTH_DEFAULT: f32 = 288.;

pub(crate) struct RightPanel {
    pub explorer: Entity<Explorer>,
    pub changes: Entity<Changes>,
    pub history: Entity<History>,
    focus: FocusHandle,
    tab: RightTab,
    shown: bool,
}

impl Focusable for RightPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl RightPanel {
    pub(crate) fn new(
        git: Entity<ProjectGit>,
        tab: RightTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let explorer = cx.new(|cx| Explorer::new(git.clone(), window, cx));
        let changes = cx.new(|cx| Changes::new(git.clone(), window, cx));
        let history = cx.new(|cx| History::new(git, window, cx));
        Self {
            explorer,
            changes,
            history,
            focus: cx.focus_handle(),
            tab,
            shown: false,
        }
    }

    pub(crate) fn set_show_excluded(&mut self, show: bool, cx: &mut Context<Self>) {
        self.explorer
            .update(cx, |explorer, cx| explorer.set_show_excluded(show, cx));
    }

    pub(crate) fn set_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        if self.shown != shown {
            self.shown = shown;
            self.sync_panes(cx);
        }
    }

    pub(crate) fn restore_tab(&mut self, tab: RightTab, cx: &mut Context<Self>) {
        if self.tab != tab {
            self.tab = tab;
            self.sync_panes(cx);
            cx.notify();
        }
    }

    fn sync_panes(&mut self, cx: &mut Context<Self>) {
        let (shown, tab) = (self.shown, self.tab);
        self.explorer.update(cx, |e, cx| {
            e.set_enabled(shown && tab == RightTab::Files, cx)
        });
        self.changes.update(cx, |c, cx| {
            c.set_enabled(shown && tab == RightTab::Changes, cx)
        });
        self.history.update(cx, |h, cx| {
            h.set_enabled(shown && tab == RightTab::Graph, cx)
        });
    }

    pub(crate) fn focus_pane(&self, window: &mut Window, cx: &mut App) {
        let focus = match (self.shown, self.tab) {
            (true, RightTab::Files) => self.explorer.focus_handle(cx),
            (true, RightTab::Graph) => self.history.focus_handle(cx),
            _ => self.focus.clone(),
        };
        window.focus(&focus, cx);
    }
}

impl Render for RightPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let pane: AnyElement = match self.tab {
            RightTab::Files => self
                .explorer
                .clone()
                .cached(StyleRefinement::default().flex_1().min_h_0().w_full())
                .into_any_element(),
            RightTab::Changes => self
                .changes
                .clone()
                .cached(StyleRefinement::default().flex_1().min_h_0().w_full())
                .into_any_element(),
            RightTab::Graph => self
                .history
                .clone()
                .cached(StyleRefinement::default().flex_1().min_h_0().w_full())
                .into_any_element(),
        };
        div()
            .id("right-panel")
            .track_focus(&self.focus)
            .role(Role::Group)
            .aria_label("Project tools")
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .child(pane)
    }
}
