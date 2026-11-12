//! SessionReview.tsx: the latest turn's recorded workspace changes, under its
//! reply, with Undo, Keep and Review. The checkpoint runs off the UI thread.
use crate::panel_kit::{format_integer, tabular};
use gpui::{
    Context, EventEmitter, FontWeight, Role, SharedString, Task, Window, div, prelude::*, px,
};
use vorkspace_core::checkpoint::{CheckpointFile, CheckpointStatus};
use vorkspace_ui::radius;
use vorkspace_ui::{file_icons::file_type_icon, theme::*};

/// The card shows this many files until expanded.
const COLLAPSED: usize = 3;

pub(crate) enum SessionReviewEvent {
    /// Review the session's changes, focused on one file when given.
    Open {
        path: Option<String>,
        turn_id: String,
    },
    /// Undo or Keep rewrote the workspace.
    Changed { cwd: String },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Keep,
    Undo,
}

pub(crate) struct SessionReview {
    session: i64,
    cwd: String,
    files: Vec<CheckpointFile>,
    turn_id: Option<String>,
    error: Option<String>,
    action_error: Option<String>,
    expanded: bool,
    acting: Option<Action>,
    /// The card represents a settled turn, so it hides while one runs.
    busy: bool,
    /// Another session is running in this project.
    undo_locked: bool,
    generation: u64,
    _load: Option<Task<()>>,
}
impl EventEmitter<SessionReviewEvent> for SessionReview {}

impl SessionReview {
    pub(crate) fn new(session: i64, cwd: String) -> Self {
        Self {
            session,
            cwd,
            files: Vec::new(),
            turn_id: None,
            error: None,
            action_error: None,
            expanded: false,
            acting: None,
            busy: false,
            undo_locked: false,
            generation: 0,
            _load: None,
        }
    }
    pub(crate) fn session(&self) -> (i64, &str) {
        (self.session, &self.cwd)
    }
    fn valid(&self) -> bool {
        !self.cwd.is_empty() && self.cwd != "~"
    }
    /// The checkout the session works in; a worktree replaces the project.
    pub(crate) fn set_cwd(&mut self, cwd: String, cx: &mut Context<Self>) {
        if self.cwd != cwd {
            self.cwd = cwd;
            self.generation += 1;
            self._load = None;
            self.publish(Ok(CheckpointStatus::default()), cx);
            self.refresh(cx);
        }
    }
    /// sessionReviewCache.startTurn: a new turn clears the card until it settles.
    pub(crate) fn set_busy(&mut self, busy: bool, cx: &mut Context<Self>) {
        if self.busy == busy {
            return;
        }
        self.busy = busy;
        // The settled turn is reread by the conversation's refresh.
        if busy {
            self.generation += 1;
            self._load = None;
            self.publish(Ok(CheckpointStatus::default()), cx);
        }
        cx.notify();
    }
    pub(crate) fn set_undo_locked(&mut self, locked: bool, cx: &mut Context<Self>) {
        if self.undo_locked != locked {
            self.undo_locked = locked;
            cx.notify();
        }
    }
    /// The card has a settled result or an error to show.
    pub(crate) fn shown(&self) -> bool {
        !self.busy && (self.error.is_some() || !self.files.is_empty())
    }
    /// Reread the recorded turn, as after a turn, Git change or reload.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        if !self.valid() || self.busy || self.acting.is_some() {
            return;
        }
        let Some(store) = vorkspace_core::checkpoint::store() else {
            return;
        };
        self.generation += 1;
        let generation = self.generation;
        let (session, cwd) = (self.session.to_string(), self.cwd.clone());
        let work = cx
            .background_executor()
            .spawn(async move { store.exclusive(|s| s.status(&session, &cwd)) });
        self._load = Some(cx.spawn(async move |this, cx| {
            let status = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation == generation && !this.busy {
                    this.publish(status, cx);
                }
            });
        }));
    }
    fn publish(&mut self, status: Result<CheckpointStatus, String>, cx: &mut Context<Self>) {
        let (files, turn_id, error) = match status {
            Ok(status) => (status.files, status.turn_id, None),
            Err(error) => (Vec::new(), None, Some(error)),
        };
        if files.len() <= COLLAPSED {
            self.expanded = false;
        }
        if self.turn_id != turn_id {
            self.action_error = None;
        }
        if self.files != files || self.error != error || self.turn_id != turn_id {
            self.files = files;
            self.turn_id = turn_id;
            self.error = error;
            cx.notify();
        }
    }
    fn run(&mut self, action: Action, expected: &str, cx: &mut Context<Self>) {
        if self.acting.is_some()
            || self.busy
            || self.turn_id.as_deref() != Some(expected)
            || (action == Action::Undo && self.undo_locked)
        {
            return;
        }
        let Some(store) = vorkspace_core::checkpoint::store() else {
            return;
        };
        self.acting = Some(action);
        self.action_error = None;
        self._load = None;
        cx.notify();
        self.generation += 1;
        let generation = self.generation;
        let (session, cwd) = (self.session.to_string(), self.cwd.clone());
        let turn_id = expected.to_owned();
        let destination = cwd.clone();
        let work = cx.background_executor().spawn(async move {
            match action {
                Action::Keep => store.keep(&session, &cwd, &turn_id, None),
                Action::Undo => store.undo(&session, &cwd, &turn_id, None),
            }
        });
        cx.spawn(async move |this, cx| {
            let status = work.await;
            let _ = this.update(cx, |this, cx| {
                this.acting = None;
                match status {
                    Ok(status) => {
                        if this.generation == generation {
                            this.publish(Ok(status), cx);
                        } else {
                            this.refresh(cx);
                        }
                        if action == Action::Undo {
                            cx.emit(SessionReviewEvent::Changed { cwd: destination });
                        }
                    }
                    Err(error) => {
                        if this.generation == generation {
                            this.action_error = Some(error);
                        }
                        this.refresh(cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open(&self, expected: &str, path: Option<String>, cx: &mut Context<Self>) {
        if !self.busy && self.turn_id.as_deref() == Some(expected) {
            cx.emit(SessionReviewEvent::Open {
                path,
                turn_id: expected.to_owned(),
            });
        }
    }
}

/// The header's text buttons: h-7 px-2.5 at 11px, ink 50%.
fn text_button(id: &'static str, label: &'static str, enabled: bool) -> gpui::Stateful<gpui::Div> {
    crate::controls::button(id, label, enabled)
        .h(px(28.))
        .px(px(10.))
        .flex()
        .items_center()
        .rounded(px(radius::CONTROL))
        .text_size(px(11.))
        .text_color(content(0.5))
        .when(!enabled, |d| d.opacity(0.35))
        .when(enabled, |d| {
            d.cursor_pointer().hover(|s| {
                s.bg(vorkspace_ui::fluid::hover(0.08))
                    .text_color(content(1.))
            })
        })
        .child(label)
}

/// DiffCounts: `+a -d` at 11px semibold, both shown even when zero.
fn counts(additions: i64, deletions: i64, gap: f32) -> gpui::Div {
    div()
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(gap))
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .font_features(tabular())
        .font_family(vorkspace_ui::fonts::MONO)
        .child(
            div()
                .text_color(vorkspace_ui::theme::diff_foreground(true))
                .child(format!("+{}", format_integer(additions))),
        )
        .child(
            div()
                .text_color(vorkspace_ui::theme::diff_foreground(false))
                .child(format!("-{}", format_integer(deletions))),
        )
}

fn card() -> gpui::Div {
    div()
        .w_full()
        .rounded(px(radius::SURFACE))
        .border_1()
        .border_color(content(0.06))
        .bg(content(0.03))
}

impl Render for SessionReview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.busy {
            return div().into_any_element();
        }
        if let Some(error) = self.error.clone() {
            return card()
                .id("session-review")
                .role(Role::Status)
                .px_3()
                .py(px(10.))
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(content(0.8))
                        .child("Couldn’t record changes"),
                )
                .child(
                    div()
                        .mt_1()
                        .text_size(px(11.))
                        .text_color(content(0.5))
                        .child(error),
                )
                .into_any_element();
        }
        let Some(turn_id) = self.turn_id.clone().filter(|_| !self.files.is_empty()) else {
            return div().into_any_element();
        };
        let undo_turn = turn_id.clone();
        let keep_turn = turn_id.clone();
        let review_turn = turn_id.clone();
        let disabled = self.acting.is_some();
        let can_undo = !self.undo_locked && self.files.iter().all(|f| f.undoable);
        let visible = if self.expanded {
            self.files.len()
        } else {
            self.files.len().min(COLLAPSED)
        };
        let hidden = self.files.len() - visible;
        let additions = self.files.iter().map(|f| f.additions).sum();
        let deletions = self.files.iter().map(|f| f.deletions).sum();
        let count = self.files.len();
        let title: SharedString = format!(
            "Changed {count} {}",
            if count == 1 { "file" } else { "files" }
        )
        .into();
        let header = div()
            .min_w_0()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(10.))
            .px_3()
            .py(px(10.))
            .child(
                div()
                    .size(px(32.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(radius::CONTAINER))
                    .bg(content(0.08))
                    .child(icon("file-diff", 16.).text_color(content(0.55))),
            )
            .child(
                div()
                    .min_w(px(84.))
                    .flex_1()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(content(0.8))
                            .child(title),
                    )
                    .child(
                        counts(additions, deletions, 6.)
                            .line_height(px(16.5))
                            .mt(px(-2.)),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .flex_wrap()
                    .max_w_full()
                    .items_center()
                    .gap(px(2.))
                    .child(
                        text_button("session-review-undo", "Undo", !disabled && can_undo)
                            .aria_label(if can_undo {
                                "Undo the recorded changes"
                            } else if self.undo_locked {
                                "Undo is unavailable while another session is running in this project"
                            } else {
                                "Undo is unavailable because the workspace was shared, the branch moved, or a recorded file changed afterward"
                            })
                            .when(!disabled && can_undo, |d| {
                                d.on_click(
                                    cx.listener(move |this, _, _, cx| this.run(Action::Undo, &undo_turn, cx)),
                                )
                            }),
                    )
                    .child(
                        text_button("session-review-keep", "Keep", !disabled)
                            .aria_label("Keep the recorded changes and dismiss this card")
                            .when(!disabled, |d| {
                                d.on_click(
                                    cx.listener(move |this, _, _, cx| this.run(Action::Keep, &keep_turn, cx)),
                                )
                            }),
                    )
                    .child(
                        div()
                            .id("session-review-open")
                            .role(Role::Button)
                            .aria_label("Review changes")
                            .map(vorkspace_ui::theme::tab_reachable)
                            .h(px(28.))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .rounded(px(radius::CONTROL))
                            .bg(content(0.08))
                            .text_size(px(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(content(0.75))
                            .cursor_pointer()
                            .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.12)).text_color(content(1.)))
                            .child("Review")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open(&review_turn, None, cx);
                            })),
                    ),
            );
        let rows = self.files[..visible].iter().enumerate().map(|(ix, file)| {
            let name = std::path::Path::new(&file.relative)
                .file_name()
                .map_or_else(
                    || file.relative.clone(),
                    |n| n.to_string_lossy().into_owned(),
                );
            let path = file.path.clone();
            let turn_id = turn_id.clone();
            let group: SharedString = format!("session-review-file-{ix}").into();
            div()
                .id(("session-review-file", ix))
                .role(Role::Button)
                .map(vorkspace_ui::theme::tab_reachable)
                .aria_label(SharedString::from(file.relative.clone()))
                .group(group.clone())
                .h(px(32.))
                .w_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .cursor_pointer()
                .text_color(content(0.65))
                .hover(|s| {
                    s.bg(vorkspace_ui::fluid::hover(0.05))
                        .text_color(content(1.))
                })
                .child(file_type_icon(&name, false, false, 15.))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .font_family(vorkspace_ui::fonts::FIXED_FONT)
                        .text_size(px(12.))
                        .child(file.relative.clone()),
                )
                .child(counts(file.additions, file.deletions, 8.))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.open(&turn_id, Some(path.clone()), cx);
                }))
        });
        let list = div()
            .id("session-review-files")
            .border_t_1()
            .border_color(content(0.05))
            .py_1()
            .when(self.expanded, |d| d.max_h(px(256.)).overflow_y_scroll())
            .children(rows);
        let expanded = self.expanded;
        card()
            .id("session-review")
            .overflow_hidden()
            .child(header)
            .when_some(self.action_error.clone(), |d, error| {
                d.child(
                    div()
                        .id("session-review-action-error")
                        .role(Role::Alert)
                        .px_3()
                        .pb_2()
                        .text_size(px(11.))
                        .text_color(status(StatusColor::Danger, 1.))
                        .child(error),
                )
            })
            .child(list)
            .when(count > COLLAPSED, |d| {
                d.child(
                    div()
                        .id("session-review-more")
                        .role(Role::Button)
                        .map(vorkspace_ui::theme::tab_reachable)
                        .aria_expanded(expanded)
                        .group("session-review-more")
                        .h(px(32.))
                        .w_full()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .border_t_1()
                        .border_color(content(0.05))
                        .px_3()
                        .cursor_pointer()
                        .text_size(px(11.))
                        .text_color(content(0.45))
                        .hover(|s| {
                            s.bg(vorkspace_ui::fluid::hover(0.05))
                                .text_color(content(0.7))
                        })
                        .child(
                            icon(
                                if expanded {
                                    "chevron-down"
                                } else {
                                    "chevron-right"
                                },
                                14.,
                            )
                            .text_color(content(0.45))
                            .group_hover("session-review-more", |s| s.text_color(content(0.7))),
                        )
                        .child(if expanded {
                            "Show fewer files".to_string()
                        } else {
                            format!(
                                "Show {hidden} more {}",
                                if hidden == 1 { "file" } else { "files" }
                            )
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.expanded = !this.expanded;
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }
}
