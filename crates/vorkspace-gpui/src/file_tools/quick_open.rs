//! Cmd+P's project-file search and `>` command palette.
use crate::input;
use gpui::{
    Context, Entity, EventEmitter, FocusHandle, Focusable, Role, Subscription, Window, div,
    prelude::*, px,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use vorkspace_core::{composer, files::ProjectFile};
use vorkspace_ui::radius;
use vorkspace_ui::theme::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {
    New,
    Project,
    Model,
    Settings,
    Sidebar,
    Panel,
    Terminal,
    Restart,
    Plan,
    Operator,
    Orchestrator,
}
const COMMANDS: &[(Command, &str, &str)] = &[
    (Command::New, "New conversation", "⌘N"),
    (Command::Project, "Open project", "⌘O"),
    (Command::Model, "Switch model", "⌘."),
    (Command::Settings, "Open settings", "⌘,"),
    (Command::Sidebar, "Toggle sidebar", "⌘B"),
    (Command::Panel, "Toggle files and changes", "⌘⇧B"),
    (Command::Terminal, "Toggle terminal", "⌘J"),
    (Command::Restart, "Restart vorkspace", "⌥⌘R"),
    (Command::Plan, "Toggle Plan mode", ""),
    (Command::Operator, "Toggle Operator", ""),
    (Command::Orchestrator, "Toggle Orchestrator", ""),
];
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Picked {
    File(String),
    Command(Command),
    Dismiss,
}
#[derive(Clone)]
struct Row {
    label: String,
    detail: String,
    item: Picked,
}
pub(crate) struct QuickOpen {
    pub(crate) search: Entity<input::Input>,
    files: Arc<Vec<ProjectFile>>,
    rows: Vec<Row>,
    active: Option<usize>,
    scroll: gpui::ScrollHandle,
    loading: bool,
    error: Option<String>,
    cancel: Arc<AtomicBool>,
    _changed: Subscription,
}
impl EventEmitter<Picked> for QuickOpen {}
impl Focusable for QuickOpen {
    fn focus_handle(&self, cx: &gpui::App) -> FocusHandle {
        self.search.focus_handle(cx)
    }
}
impl Drop for QuickOpen {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
/// The last listing of the most recently opened roots. Quick Open is built
/// fresh on every open; without this each open showed nothing until a full
/// rescan of the project finished. A cached listing is shown at once and
/// replaced when the rescan lands, so new and deleted files still appear.
const CACHED_ROOTS: usize = 4;
static LISTINGS: std::sync::Mutex<Vec<(String, Arc<Vec<ProjectFile>>)>> =
    std::sync::Mutex::new(Vec::new());

fn cached_listing(cwd: &str) -> Option<Arc<Vec<ProjectFile>>> {
    let listings = LISTINGS.lock().ok()?;
    listings
        .iter()
        .find(|(root, _)| root == cwd)
        .map(|(_, files)| files.clone())
}

fn remember_listing(cwd: &str, files: Arc<Vec<ProjectFile>>) {
    if let Ok(mut listings) = LISTINGS.lock() {
        listings.retain(|(root, _)| root != cwd);
        listings.insert(0, (cwd.to_owned(), files));
        listings.truncate(CACHED_ROOTS);
    }
}

impl QuickOpen {
    pub(crate) fn new(cwd: String, palette: bool, cx: &mut Context<Self>) -> Self {
        let cached = cached_listing(&cwd);
        let root = cwd.clone();
        let search = cx.new(|cx| {
            input::Input::field(
                "Go to file or run a command",
                "Search files by name…",
                content(0.35),
                cx,
            )
        });
        if palette {
            search.update(cx, |input, cx| input.set_text(">".into(), cx));
        }
        let changed = cx.subscribe(&search, |this, _, _: &input::Changed, cx| {
            this.rank(false, cx);
            cx.notify();
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let work = cx.background_executor().spawn(async move {
            vorkspace_core::files::list_project_files_sync_cancellable(&cwd, Some(&flag))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(files) => {
                        let files = Arc::new(files);
                        remember_listing(&root, files.clone());
                        this.files = files;
                    }
                    Err(e) => this.error = Some(e),
                };
                this.rank(true, cx);
                cx.notify();
            });
        })
        .detach();
        let mut this = Self {
            search,
            files: cached.clone().unwrap_or_default(),
            rows: Vec::new(),
            active: None,
            scroll: gpui::ScrollHandle::new(),
            loading: cached.is_none(),
            error: None,
            cancel,
            _changed: changed,
        };
        this.rank(false, cx);
        this
    }
    fn rank(&mut self, preserve_selection: bool, cx: &gpui::App) {
        let selected = self
            .active
            .filter(|_| preserve_selection)
            .and_then(|ix| self.rows.get(ix))
            .map(|row| row.item.clone());
        let query = self.search.read(cx).content.trim();
        self.rows = if let Some(query) = query.strip_prefix('>') {
            let mut commands: Vec<_> = COMMANDS
                .iter()
                .filter_map(|(command, label, hint)| {
                    composer::score(query.trim(), label).map(|score| {
                        (
                            score,
                            Row {
                                label: (*label).into(),
                                detail: (*hint).into(),
                                item: Picked::Command(*command),
                            },
                        )
                    })
                })
                .collect();
            commands.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
            commands.into_iter().map(|(_, row)| row).collect()
        } else {
            composer::rank_files(&self.files, query, 80)
                .into_iter()
                .map(|file| Row {
                    label: file.name.clone(),
                    detail: file.relative.clone(),
                    item: Picked::File(file.path.clone()),
                })
                .collect()
        };
        self.active = match selected {
            Some(item) => self.rows.iter().position(|row| row.item == item),
            None => (!self.rows.is_empty()).then_some(0),
        };
        if let Some(active) = self.active {
            self.scroll.scroll_to_item(active);
        }
    }
    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if !self.rows.is_empty() {
            let len = self.rows.len();
            let active = self.active.map_or_else(
                || if delta < 0 { len - 1 } else { 0 },
                |ix| (ix as isize + delta).rem_euclid(len as isize) as usize,
            );
            self.active = Some(active);
            self.scroll.scroll_to_item(active);
            cx.notify();
        }
    }
    fn pick(&self, item: &Picked, cx: &mut Context<Self>) {
        if !self.search.read(cx).is_composing() && self.rows.iter().any(|row| &row.item == item) {
            cx.emit(item.clone());
        }
    }
}
impl Render for QuickOpen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.search.read(cx).content.trim().to_owned();
        // Point at the other mode, not the one already open. Backspace only
        // returns to files while the prefix is all there is to delete.
        let mode_hint = if !query.starts_with('>') {
            Some((">", "commands"))
        } else if query == ">" {
            Some(("⌫", "files"))
        } else {
            None
        };
        let mut hints = vec![("↑↓", "navigate"), ("↵", "open"), ("esc", "close")];
        hints.extend(mode_hint);
        let rows: Vec<_> = self
            .rows
            .iter()
            .enumerate()
            .map(|(ix, row)| {
                let item = row.item.clone();
                let command = matches!(item, Picked::Command(_));
                div()
                    .id(("quick-open-row", ix))
                    .role(Role::ListItem)
                    .aria_label(if command {
                        format!("{} {}", row.label, row.detail)
                    } else {
                        row.detail.to_string()
                    })
                    .px_3()
                    // One line: a command's name and its keys as chips, a
                    // file's name and the folder it sits in.
                    .py(px(7.))
                    .rounded(px(radius::CONTROL))
                    .flex()
                    .gap_2p5()
                    .items_center()
                    .bg(if self.active == Some(ix) {
                        content(0.1)
                    } else {
                        content(0.)
                    })
                    .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.07)))
                    .when(!command, |d| {
                        d.child(vorkspace_ui::file_icons::file_type_icon(
                            &row.label, false, false, 16.,
                        ))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_baseline()
                            .gap_2()
                            .text_size(px(13.))
                            .text_color(content(if self.active == Some(ix) { 1. } else { 0.85 }))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .max_w_full()
                                    .truncate()
                                    .child(row.label.clone()),
                            )
                            .when(!command, |d| {
                                let folder = std::path::Path::new(&row.detail)
                                    .parent()
                                    .map(|p| p.to_string_lossy().into_owned())
                                    .unwrap_or_default();
                                d.when(!folder.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .min_w_0()
                                            .text_size(px(12.))
                                            .text_color(content(0.4))
                                            .truncate()
                                            .child(folder),
                                    )
                                })
                            }),
                    )
                    .when(command && !row.detail.is_empty(), |d| {
                        d.child(vorkspace_ui::fluid::kbd(row.detail.clone()))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.pick(&item, cx)))
                    .into_any_element()
            })
            .collect();
        div()
            .id("quick-open")
            .role(Role::Dialog)
            .aria_label("Go to file or run a command")
            .w_full()
            .flex()
            .flex_col()
            .capture_action(cx.listener(|this, _: &input::Up, _, cx| {
                this.step(-1, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &input::Down, _, cx| {
                this.step(1, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &input::Submit, _, cx| {
                if let Some(row) = this.active.and_then(|ix| this.rows.get(ix)) {
                    this.pick(&row.item, cx);
                }
                cx.stop_propagation();
            }))
            .on_action(
                cx.listener(|_, _: &vorkspace_ui::actions::DismissMenu, _, cx| {
                    cx.emit(Picked::Dismiss);
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(content(0.08))
                    .flex()
                    .items_center()
                    .gap_2p5()
                    .child(icon("search", 16.).text_color(content(0.5)))
                    .child(div().flex_1().min_w_0().child(self.search.clone())),
            )
            .child(
                div()
                    .id("quick-open-results")
                    .role(Role::ListBox)
                    .aria_label("Results")
                    .p_1p5()
                    .max_h(px(380.))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .children(rows)
                    .when(self.rows.is_empty(), |d| {
                        d.child(
                            div()
                                .px_3()
                                .py_6()
                                .text_size(px(13.))
                                .text_color(content(0.45))
                                .child(if self.loading {
                                    "Loading project files…".to_owned()
                                } else {
                                    self.error.clone().unwrap_or_else(|| "No results".into())
                                }),
                        )
                    }),
            )
            .child(
                div()
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(content(0.07))
                    .child(vorkspace_ui::fluid::key_hints(&hints)),
            )
    }
}

#[cfg(test)]
mod listing_tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn reopening_shows_the_last_listing_at_once_and_refreshes_it(cx: &mut TestAppContext) {
        let dir = std::env::temp_dir().join(format!(
            "vorkspace-quick-open-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("later.rs"), "").unwrap();
        let cwd = dir.to_string_lossy().into_owned();
        cx.update(input::bind);
        let first = cx.update(|cx| cx.new(|cx| QuickOpen::new(cwd.clone(), false, cx)));
        cx.run_until_parked();
        assert_eq!(first.read_with(cx, |q, _| q.files.len()), 1);
        std::fs::write(dir.join("earlier.rs"), "").unwrap();
        let second = cx.update(|cx| cx.new(|cx| QuickOpen::new(cwd.clone(), false, cx)));
        // Before the rescan runs, the previous listing is already there.
        second.read_with(cx, |q, _| {
            assert_eq!(q.files.len(), 1);
            assert!(!q.loading);
            assert!(!q.rows.is_empty());
        });
        cx.run_until_parked();
        second.read_with(cx, |q, _| {
            assert_eq!(q.files.len(), 2);
            assert_eq!(q.active, Some(1));
            assert_eq!(q.rows[q.active.unwrap()].label, "later.rs");
        });
        let _ = std::fs::remove_dir_all(&dir);
    }
}
