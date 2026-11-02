//! SidebarWorktreeSwitcher.tsx's Working copies flyout. The composer keeps its
//! own branch picker; this panel selects the project's workspace scope.
use crate::{input, popover};
use gpui::{
    Context, Entity, EventEmitter, FocusHandle, Focusable, Role, ScrollHandle, Subscription,
    Window, div, prelude::*, px,
};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};
use vorkspace_core::{
    files::path_key,
    git::{self, Snapshot},
};
use vorkspace_runtime::runtime::{Database, locked};
use vorkspace_ui::theme::*;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct WorktreeFocus {
    pub path: String,
    pub branch: Option<String>,
}
#[derive(Clone)]
pub(crate) struct TabSession {
    pub id: i64,
    pub tab: i64,
    pub cwd: String,
    pub workspace: Option<String>,
    pub busy: bool,
}
pub(crate) enum Event {
    Selected(Option<WorktreeFocus>),
    Metadata(Vec<(i64, Option<String>)>),
    Dismissed,
}
#[derive(Clone)]
struct Row {
    path: String,
    branch: Option<String>,
    main: bool,
    label: String,
    detail: String,
}
fn rows(cwd: &str, state: Option<&Snapshot>, query: &str) -> Vec<Row> {
    let main = state.and_then(|s| s.worktrees.iter().find(|t| t.main));
    let mut rows = vec![Row {
        path: main.map_or_else(|| cwd.to_owned(), |t| t.path.clone()),
        branch: main.and_then(|t| t.branch.clone()),
        main: true,
        label: main
            .and_then(|t| t.branch.clone())
            .unwrap_or_else(|| "Project folder".into()),
        detail: "Project folder · all sessions".into(),
    }];
    rows.extend(
        state
            .into_iter()
            .flat_map(|s| &s.worktrees)
            .filter(|t| !t.main && !t.missing)
            .map(|t| Row {
                path: t.path.clone(),
                branch: t.branch.clone(),
                main: false,
                label: t.branch.clone().unwrap_or_else(|| {
                    format!("Detached {}", t.head.chars().take(7).collect::<String>())
                }),
                detail: pretty_path(&t.path),
            }),
    );
    let needle = query.trim().to_lowercase();
    rows.retain(|r| {
        format!("{}\n{}\n{}", r.label, r.detail, r.path)
            .to_lowercase()
            .contains(&needle)
    });
    rows
}
fn pretty_path(path: &str) -> String {
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
        && let Ok(rest) = Path::new(path).strip_prefix(home)
    {
        return format!("~/{}", rest.display());
    }
    path.into()
}
#[derive(Default)]
struct Tabs {
    tabs: HashSet<i64>,
    busy: bool,
}
fn tab_stats(
    sessions: &[TabSession],
    paths: Option<&HashMap<i64, Option<String>>>,
    project: &str,
) -> HashMap<String, Tabs> {
    let mut tabs: HashMap<String, Tabs> = HashMap::new();
    for session in sessions {
        let path = session.workspace.as_deref().unwrap_or_else(|| {
            match paths.and_then(|paths| paths.get(&session.id)) {
                // An explicit cleared membership returns to the project folder.
                Some(path) => path.as_deref().unwrap_or(project),
                None => &session.cwd,
            }
        });
        let stats = tabs.entry(path_key(path)).or_default();
        stats.tabs.insert(session.tab);
        stats.busy |= session.busy;
    }
    tabs
}
pub(crate) struct WorktreeSwitcher {
    cwd: String,
    focused: Option<WorktreeFocus>,
    state: Option<Snapshot>,
    search: Entity<input::Input>,
    active: usize,
    scroll: ScrollHandle,
    creating: bool,
    load_error: Option<String>,
    error: Option<String>,
    tabs: HashMap<String, Tabs>,
    _changed: Subscription,
}
impl EventEmitter<Event> for WorktreeSwitcher {}
impl WorktreeSwitcher {
    pub(crate) fn new(
        cwd: String,
        focused: Option<WorktreeFocus>,
        sessions: Vec<TabSession>,
        ids: Vec<i64>,
        store: Database,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            let mut input = input::Input::field(
                "Search working copies",
                "Search or create a worktree...",
                content(0.35),
                cx,
            );
            input.set_font_size(13.);
            input
        });
        let changed = cx.subscribe(&search, |this, _, _: &input::Changed, cx| {
            this.active = 0;
            this.scroll.scroll_to_item(0);
            this.error = None;
            cx.notify();
        });
        let state = vorkspace_model::git_cache::get(&cwd);
        let tabs = tab_stats(&sessions, None, &cwd);
        let folder = cwd.clone();
        let work = cx
            .background_executor()
            .spawn(async move { vorkspace_model::git_cache::load(Path::new(&folder)) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(state) => {
                        this.state = Some(state);
                        this.load_error = None;
                    }
                    Err(error) => this.load_error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        // Session membership is metadata, not a reason to load every agent's
        // settings (which restores full desktop transcripts and turn receipts).
        // It must also never hold up the independent Git list.
        if !ids.is_empty() {
            let work = cx.background_executor().spawn(async move {
                let paths = locked(&store)?.session_worktrees()?;
                Ok::<_, String>(
                    ids.into_iter()
                        .map(|id| (id, paths.get(&id).cloned().flatten()))
                        .collect::<Vec<_>>(),
                )
            });
            cx.spawn(async move |this, cx| {
                let result = work.await;
                let _ = this.update(cx, |this, cx| {
                    match result {
                        Ok(paths) => {
                            let workspaces: HashMap<_, _> = paths.iter().cloned().collect();
                            this.tabs = tab_stats(&sessions, Some(&workspaces), &this.cwd);
                            cx.emit(Event::Metadata(paths));
                        }
                        Err(error) => this.error = Some(error),
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        Self {
            cwd,
            focused,
            state,
            search,
            active: 0,
            scroll: ScrollHandle::new(),
            creating: false,
            load_error: None,
            error: None,
            tabs,
            _changed: changed,
        }
    }
    pub(crate) fn focus(&self, cx: &gpui::App) -> FocusHandle {
        self.search.focus_handle(cx)
    }
    fn query(&self, cx: &gpui::App) -> String {
        self.search.read(cx).content.trim().to_owned()
    }
    fn can_create(&self, cx: &gpui::App) -> bool {
        self.state.is_some()
            && self.load_error.is_none()
            && !self.query(cx).is_empty()
            && rows(&self.cwd, self.state.as_ref(), &self.query(cx)).is_empty()
    }
    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.creating {
            return;
        }
        let count = rows(&self.cwd, self.state.as_ref(), &self.query(cx)).len();
        self.active = self
            .active
            .saturating_add_signed(delta)
            .min(count.saturating_sub(1));
        self.scroll.scroll_to_item(self.active);
        cx.notify();
    }
    fn pick(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.creating {
            return;
        }
        if let Some(row) = rows(&self.cwd, self.state.as_ref(), &self.query(cx)).get(index) {
            cx.emit(Event::Selected((!row.main).then(|| WorktreeFocus {
                path: row.path.clone(),
                branch: row.branch.clone(),
            })));
        } else if self.can_create(cx) {
            self.create(cx);
        }
    }
    fn create(&mut self, cx: &mut Context<Self>) {
        if !self.can_create(cx) || self.creating {
            return;
        }
        let branch = self.query(cx);
        let cwd = self.focused.as_ref().map_or(&self.cwd, |f| &f.path).clone();
        let project = self.cwd.clone();
        self.creating = true;
        self.error = None;
        let work = cx.background_executor().spawn(async move {
            let tree = git::create_named_worktree(Path::new(&cwd), &branch)?;
            let setup = vorkspace_core::worktree_management::prepare(
                Path::new(&project),
                Path::new(&tree.path),
                &Default::default(),
            );
            Ok::<_, String>((tree, setup))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.creating = false;
                match result {
                    Ok((tree, setup)) => {
                        this.error = setup;
                        cx.emit(Event::Selected(Some(WorktreeFocus {
                            path: tree.path,
                            branch: tree.branch,
                        })));
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
impl Render for WorktreeSwitcher {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = rows(&self.cwd, self.state.as_ref(), &self.query(cx));
        let empty = rows.is_empty();
        let can_create = self.can_create(cx);
        div()
            .id("worktree-switcher")
            .debug_selector(|| "worktree-switcher".into())
            .role(Role::Dialog)
            .aria_label("Working copies")
            .key_context("WorktreeSwitcher")
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .capture_action(cx.listener(|this, _: &input::Down, _, cx| {
                this.step(1, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &input::Up, _, cx| {
                this.step(-1, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &input::Submit, _, cx| {
                this.pick(this.active, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|this, _: &popover::Cancel, _, cx| {
                if !this.creating {
                    cx.emit(Event::Dismissed);
                }
                cx.stop_propagation();
            }))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    if !this.creating {
                        cx.emit(Event::Dismissed);
                    }
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .debug_selector(|| "worktree-switcher-search".into())
                    .h(px(44.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .border_b_1()
                    .border_color(content(0.07))
                    .px_3()
                    .child(icon("search-01", 16.).text_color(content(0.45)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .when(self.creating, |d| d.opacity(0.6))
                            .child(self.search.clone()),
                    ),
            )
            .child(
                div()
                    .id("worktree-switcher-list")
                    .role(Role::ListBox)
                    .aria_label("Working copies")
                    .min_h_0()
                    .max_h(px(314.))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .p(px(6.))
                    .flex()
                    .flex_col()
                    .when(self.state.is_none() && self.load_error.is_none(), |d| {
                        d.child(
                            div()
                                .debug_selector(|| "worktree-switcher-loading".into())
                                .flex()
                                .items_center()
                                .gap_2()
                                .p_2()
                                .text_size(px(12.))
                                .text_color(content(0.5))
                                .child(icon("loader", 14.))
                                .child("Loading working copies…"),
                        )
                    })
                    .children(rows.into_iter().enumerate().map(|(index, row)| {
                        let selected = if row.main {
                            self.focused.is_none()
                        } else {
                            self.focused
                                .as_ref()
                                .is_some_and(|focus| path_key(&focus.path) == path_key(&row.path))
                        };
                        let stats = self.tabs.get(&path_key(&row.path));
                        div()
                            .id(("working-copy", index))
                            .debug_selector(|| format!("working-copy-{index}"))
                            .role(Role::ListBoxOption)
                            .aria_label(format!("{} · {}", row.label, row.detail))
                            .aria_description(row.path)
                            .aria_selected(selected)
                            .flex_shrink_0()
                            .w_full()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded_md()
                            .px_2()
                            .py(px(6.))
                            .text_color(content(1.))
                            .when(self.creating, |d| d.opacity(0.4))
                            .when(!self.creating, |d| d.cursor_pointer())
                            .map(|d| {
                                if self.active == index {
                                    d.bg(content(0.1))
                                } else {
                                    d.hover(|s| s.bg(content(0.05)))
                                }
                            })
                            .child(
                                icon(
                                    if row.main {
                                        "git-branch"
                                    } else {
                                        "folder-tree"
                                    },
                                    14.,
                                )
                                .text_color(content(0.5)),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(12.))
                                            .line_height(px(18.))
                                            .child(row.label),
                                    )
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(10.))
                                            .line_height(px(15.))
                                            .text_color(content(0.4))
                                            .child(row.detail),
                                    ),
                            )
                            .when_some(stats.filter(|s| !s.tabs.is_empty()), |d, stats| {
                                d.child(
                                    div()
                                        .id(("working-copy-stats", index))
                                        .role(Role::Label)
                                        .aria_label(format!(
                                            "{} open tab{}{}",
                                            stats.tabs.len(),
                                            if stats.tabs.len() == 1 { "" } else { "s" },
                                            if stats.busy { ", working" } else { "" }
                                        ))
                                        .flex_shrink_0()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .text_size(px(11.))
                                        .text_color(content(0.4))
                                        .when(stats.busy, |d| {
                                            d.child(
                                                div()
                                                    .size(px(6.))
                                                    .rounded_full()
                                                    .bg(accent_color()),
                                            )
                                        })
                                        .child(stats.tabs.len().to_string()),
                                )
                            })
                            .when(selected, |d| {
                                d.child(icon("check", 14.).text_color(content(1.)))
                            })
                            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                if *hovered && !this.creating {
                                    this.active = index;
                                    cx.notify();
                                }
                            }))
                            .on_click(cx.listener(move |this, _, _, cx| this.pick(index, cx)))
                    }))
                    .when(self.state.is_some() && empty, |d| {
                        d.child(
                            div()
                                .px(px(10.))
                                .py_5()
                                .text_center()
                                .text_size(px(12.))
                                .text_color(content(0.45))
                                .child("No matching working copies"),
                        )
                    }),
            )
            .when_some(
                self.error.clone().or_else(|| self.load_error.clone()),
                |d, error| {
                    d.child(
                        div()
                            .id("worktree-switcher-error")
                            .role(Role::Alert)
                            .px_2()
                            .py_2()
                            .text_size(px(11.))
                            .text_color(gpui::rgb(0xf87171))
                            .child(error),
                    )
                },
            )
            .when(can_create, |d| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .border_t_1()
                        .border_color(content(0.07))
                        .p(px(6.))
                        .child(
                            div()
                                .id("create-sidebar-worktree")
                                .debug_selector(|| "create-sidebar-worktree".into())
                                .role(Role::Button)
                                .aria_label(format!("Create worktree {}", self.query(cx)))
                                .h(px(36.))
                                .w_full()
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .rounded_lg()
                                .px(px(10.))
                                .text_size(px(13.))
                                .text_color(content(0.75))
                                .when(self.creating, |d| d.opacity(0.6))
                                .when(!self.creating, |d| {
                                    d.cursor_pointer()
                                        .hover(|s| s.bg(content(0.08)).text_color(content(1.)))
                                })
                                .child(icon(if self.creating { "loader" } else { "plus" }, 16.))
                                .child(format!("Create worktree {}", self.query(cx)))
                                .on_click(cx.listener(|this, _, _, cx| this.create(cx))),
                        ),
                )
            })
    }
}

#[cfg(test)]
mod tests;
