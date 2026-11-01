//! The composer's searchable BranchPicker and existing-worktree list.
use crate::{input, popover, theme::*};
use gpui::{
    AnyElement, Context, Entity, EventEmitter, FocusHandle, Focusable, Role, Subscription, Window,
    div, prelude::*, px,
};
use std::path::PathBuf;
use vorkspace_core::git::{self, Snapshot};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Branch,
    Base,
    Worktree,
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use std::{cell::RefCell, rc::Rc};

    #[gpui::test]
    fn clicking_base_choices_keeps_the_exact_remote_or_local_ref(cx: &mut TestAppContext) {
        let cwd = "/nonexistent/vorkspace-base-picker-test";
        vorkspace_model::git_cache::remember(
            cwd,
            &Snapshot {
                branches: vec![
                    git::Branch {
                        name: "origin/main".into(),
                        remote: None,
                    },
                    git::Branch {
                        name: "main".into(),
                        remote: Some("origin".into()),
                    },
                ],
                ..Default::default()
            },
        );
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| GitPicker::new(cwd.into(), Mode::Base, None, cx))
            })
            .unwrap()
        });
        let picker = window.update(cx, |_, _, cx| cx.entity()).unwrap();
        let picked = Rc::new(RefCell::new(Vec::new()));
        let events = picked.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&picker, move |_, event: &Picked, _| {
                if let Picked::Base(base) = event {
                    events.borrow_mut().push(base.clone());
                }
            })
        });
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        for selector in ["git-option-0", "git-option-1"] {
            let bounds = visual.debug_bounds(selector).unwrap();
            visual.simulate_click(bounds.center(), Default::default());
            visual.run_until_parked();
        }
        assert_eq!(
            *picked.borrow(),
            ["refs/heads/origin/main", "refs/remotes/origin/main"]
        );
    }

    #[gpui::test]
    fn clicking_a_remote_tracking_row_checks_out_the_existing_local_branch(
        cx: &mut TestAppContext,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(cwd)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.invalid"]);
        std::fs::write(cwd.join("file"), "initial").unwrap();
        git(&["add", "file"]);
        git(&["-c", "commit.gpgsign=false", "commit", "-m", "initial"]);
        git(&["remote", "add", "origin", "/unused"]);
        git(&["remote", "add", "other", "/unused"]);
        git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(&["update-ref", "refs/remotes/other/main", "HEAD"]);
        git(&["branch", "--set-upstream-to=origin/main", "main"]);
        git(&["switch", "-c", "feature"]);
        let mut state = git::snapshot(cwd).unwrap();
        state.branches.sort_by_key(|branch| branch.remote.is_some());
        vorkspace_model::git_cache::remember(&cwd.to_string_lossy(), &state);
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| GitPicker::new(cwd.to_owned(), Mode::Branch, None, cx))
            })
            .unwrap()
        });
        let picker = window.update(cx, |_, _, cx| cx.entity()).unwrap();
        let changed = Rc::new(RefCell::new(false));
        let event_changed = changed.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&picker, move |_, event: &Picked, _| {
                if matches!(event, Picked::Changed) {
                    *event_changed.borrow_mut() = true;
                }
            })
        });
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        // The snapshot refresh can reorder rows, so select from the rendered state.
        visual.run_until_parked();
        for remote in ["other", "origin"] {
            let ix = window
                .update(visual, |view, _, cx| {
                    view.choices(cx)
                        .iter()
                        .position(|choice| matches!(choice, Choice::Branch(b) if b.remote.as_deref() == Some(remote) && b.name == "main"))
                        .unwrap()
                })
                .unwrap();
            let selector = [
                "git-option-0",
                "git-option-1",
                "git-option-2",
                "git-option-3",
            ][ix];
            let bounds = visual.debug_bounds(selector).unwrap();
            visual.simulate_click(bounds.center(), Default::default());
            visual.run_until_parked();
            if remote == "other" {
                assert!(visual.debug_bounds("git-picker-error").is_some());
                window
                    .update(visual, |view, _, _| {
                        assert!(
                            view.error
                                .as_deref()
                                .is_some_and(|error| error.contains("does not track"))
                        );
                    })
                    .unwrap();
                assert!(!*changed.borrow());
                assert_eq!(
                    git::snapshot(cwd).unwrap().current.as_deref(),
                    Some("feature")
                );
            }
        }
        window
            .update(visual, |view, _, _| {
                assert!(view.error.is_none(), "{:?}", view.error)
            })
            .unwrap();
        assert!(*changed.borrow());
        assert_eq!(git::snapshot(cwd).unwrap().current.as_deref(), Some("main"));
    }

    #[gpui::test]
    fn all_picker_modes_start_with_cached_choices_and_keep_them_after_refresh_failure(
        cx: &mut TestAppContext,
    ) {
        let cwd = "/nonexistent/vorkspace-cached-git-picker";
        vorkspace_model::git_cache::remember(
            cwd,
            &Snapshot {
                current: Some("main".into()),
                branches: vec![
                    git::Branch {
                        name: "main".into(),
                        remote: None,
                    },
                    git::Branch {
                        name: "feature".into(),
                        remote: None,
                    },
                ],
                worktrees: vec![git::Worktree {
                    path: cwd.into(),
                    branch: Some("main".into()),
                    main: true,
                    ..Default::default()
                }],
                ..Default::default()
            },
        );
        let pickers: Vec<_> = [Mode::Branch, Mode::Base, Mode::Worktree]
            .into_iter()
            .map(|mode| {
                let picker = cx.new(|cx| GitPicker::new(cwd.into(), mode, None, cx));
                picker.update(cx, |view, cx| {
                    assert!(
                        view.state.is_some(),
                        "choices are available before any background work"
                    );
                    assert_eq!(
                        view.choices(cx).len(),
                        if mode == Mode::Worktree { 1 } else { 2 }
                    );
                });
                picker
            })
            .collect();
        cx.run_until_parked();
        for picker in pickers {
            picker.update(cx, |view, cx| {
                assert!(view.error.is_some());
                assert!(
                    !view.choices(cx).is_empty(),
                    "a failed refresh retains usable choices"
                );
            });
        }
        assert!(vorkspace_model::git_cache::get(cwd).is_some());
    }
}
pub(crate) enum Picked {
    Changed,
    Base(String),
    Worktree(String),
    Dismissed,
}
pub(crate) struct GitPicker {
    cwd: PathBuf,
    mode: Mode,
    selected: Option<String>,
    state: Option<Snapshot>,
    search: Entity<input::Input>,
    active: usize,
    busy: bool,
    existing_only: bool,
    error: Option<String>,
    _changed: Subscription,
}
#[derive(Clone)]
enum Choice {
    Branch(git::Branch),
    Create(String),
    Worktree(git::Worktree),
}
impl EventEmitter<Picked> for GitPicker {}
impl GitPicker {
    pub(crate) fn new(
        cwd: PathBuf,
        mode: Mode,
        selected: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            input::Input::field(
                if mode == Mode::Worktree {
                    "Search working copies"
                } else {
                    "Search branches"
                },
                if mode == Mode::Worktree {
                    "Search working copies…"
                } else {
                    "Search branches…"
                },
                content(0.4),
                cx,
            )
        });
        let changed = cx.subscribe(&search, |this, _, _: &input::Changed, cx| {
            this.active = 0;
            cx.notify();
        });
        let state = vorkspace_model::git_cache::get(&cwd.to_string_lossy());
        let load_cwd = cwd.clone();
        let work = cx
            .background_executor()
            .spawn(async move { vorkspace_model::git_cache::load(&load_cwd) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(state) => this.state = Some(state),
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        Self {
            cwd,
            mode,
            selected,
            state,
            search,
            active: 0,
            busy: false,
            existing_only: false,
            error: None,
            _changed: changed,
        }
    }
    pub(crate) fn focus(&self, cx: &gpui::App) -> FocusHandle {
        self.search.focus_handle(cx)
    }
    pub(crate) fn mode(&self) -> Mode {
        self.mode
    }
    /// WorkspacePicker's submenu contains available linked copies; the parent
    /// already offers the project folder through Current checkout.
    pub(crate) fn existing_worktrees(mut self) -> Self {
        self.existing_only = true;
        self
    }
    fn choices(&self, cx: &gpui::App) -> Vec<Choice> {
        let query = self.search.read(cx).content.trim().to_owned();
        let needle = query.to_lowercase();
        let Some(state) = &self.state else {
            return Vec::new();
        };
        if self.mode == Mode::Worktree {
            return state
                .worktrees
                .iter()
                .filter(|tree| !self.existing_only || (!tree.main && !tree.missing))
                .filter(|tree| {
                    format!(
                        "{} {}",
                        tree.branch.as_deref().unwrap_or("detached"),
                        tree.path
                    )
                    .to_lowercase()
                    .contains(&needle)
                })
                .cloned()
                .map(Choice::Worktree)
                .collect();
        }
        let mut choices: Vec<_> = state
            .branches
            .iter()
            .filter(|branch| {
                format!("{} {}", branch.name, branch.remote.as_deref().unwrap_or(""))
                    .to_lowercase()
                    .contains(&needle)
            })
            .cloned()
            .map(Choice::Branch)
            .collect();
        if self.mode == Mode::Branch
            && !query.is_empty()
            && !state
                .branches
                .iter()
                .any(|b| b.remote.is_none() && b.name == query)
        {
            choices.push(Choice::Create(query));
        }
        choices
    }
    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.active = self
            .active
            .saturating_add_signed(delta)
            .min(self.choices(cx).len().saturating_sub(1));
        cx.notify();
    }
    fn pick(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(choice) = self.choices(cx).get(ix).cloned() else {
            return;
        };
        match choice {
            Choice::Worktree(tree) => {
                if !tree.missing {
                    cx.emit(Picked::Worktree(tree.path));
                }
            }
            Choice::Branch(branch) if self.mode == Mode::Base => {
                cx.emit(Picked::Base(branch.remote.map_or_else(
                    || format!("refs/heads/{}", branch.name),
                    |remote| format!("refs/remotes/{remote}/{}", branch.name),
                )));
            }
            Choice::Branch(branch)
                if branch.remote.is_none()
                    && self.state.as_ref().is_some_and(|s| !s.detached)
                    && self.state.as_ref().and_then(|s| s.current.as_ref())
                        == Some(&branch.name) =>
            {
                cx.emit(Picked::Dismissed)
            }
            choice => {
                self.busy = true;
                self.error = None;
                let cwd = self.cwd.clone();
                let work = cx.background_executor().spawn(async move {
                    match choice {
                        Choice::Branch(branch) => git::checkout(&cwd, &branch),
                        Choice::Create(name) => git::create_branch(&cwd, &name),
                        _ => unreachable!(),
                    }
                });
                cx.spawn(async move |this, cx| {
                    let result = work.await;
                    let _ = this.update(cx, |this, cx| {
                        this.busy = false;
                        match result {
                            Ok(()) => cx.emit(Picked::Changed),
                            Err(error) => this.error = Some(error),
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }
}
impl gpui::Render for GitPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let choices = self.choices(cx);
        let empty = choices.is_empty();
        let rows: Vec<AnyElement> = choices
            .into_iter()
            .enumerate()
            .map(|(ix, choice)| {
                let (label, detail, selected, disabled, path) = match choice {
                    Choice::Branch(branch) => {
                        let full = branch
                            .remote
                            .as_ref()
                            .map_or(branch.name.clone(), |r| format!("{r}/{}", branch.name));
                        let selected = if self.mode == Mode::Base {
                            let reference = branch.remote.as_ref().map_or_else(
                                || format!("refs/heads/{}", branch.name),
                                |remote| format!("refs/remotes/{remote}/{}", branch.name),
                            );
                            self.selected.as_deref().is_some_and(|selected| {
                                selected == reference
                                    || (!selected.starts_with("refs/") && selected == full)
                            })
                        } else {
                            branch.remote.is_none()
                                && self.state.as_ref().is_some_and(|s| !s.detached)
                                && self.state.as_ref().and_then(|s| s.current.as_ref())
                                    == Some(&branch.name)
                        };
                        (branch.name, branch.remote, selected, false, "git-branch")
                    }
                    Choice::Create(name) => (
                        format!("Create branch ‘{name}’"),
                        None,
                        false,
                        false,
                        "plus",
                    ),
                    Choice::Worktree(tree) => {
                        let selected = if self.existing_only {
                            self.selected.as_deref().is_some_and(|path| {
                                vorkspace_core::files::path_key(path)
                                    == vorkspace_core::files::path_key(&tree.path)
                            })
                        } else {
                            std::fs::canonicalize(&tree.path).ok().is_some_and(|path| {
                                Some(path) == std::fs::canonicalize(&self.cwd).ok()
                            })
                        };
                        (
                            tree.branch.unwrap_or_else(|| {
                                format!(
                                    "Detached {}",
                                    tree.head.chars().take(7).collect::<String>()
                                )
                            }),
                            Some(if tree.main {
                                "Project folder".into()
                            } else {
                                tree.path
                            }),
                            selected,
                            tree.missing,
                            if tree.main {
                                "git-branch"
                            } else {
                                "folder-tree"
                            },
                        )
                    }
                };
                div()
                    .id(("git-option", ix))
                    .debug_selector(move || format!("git-option-{ix}"))
                    .role(Role::ListBoxOption)
                    .aria_label(label.clone())
                    .aria_selected(selected)
                    .flex_shrink_0()
                    .min_h(px(32.))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_color(content(if disabled || self.busy { 0.35 } else { 0.9 }))
                    .when(ix == self.active, |d| d.bg(content(0.08)))
                    .child(icon(path, 14.).text_color(content(0.5)))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .child(div().truncate().text_size(px(12.)).child(label))
                            .when_some(detail, |d, detail| {
                                d.child(
                                    div()
                                        .truncate()
                                        .text_size(px(10.))
                                        .text_color(content(0.4))
                                        .child(detail),
                                )
                            }),
                    )
                    .when(selected, |d| d.child(icon("check-2", 14.)))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.active = ix;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| this.pick(ix, cx)))
                    .into_any_element()
            })
            .collect();
        let existing_only = self.existing_only;
        div()
            .id("git-picker")
            .debug_selector(move || {
                if existing_only {
                    "existing-worktrees-submenu".into()
                } else {
                    "git-picker".into()
                }
            })
            .role(Role::Dialog)
            .aria_label(if existing_only {
                "Existing worktrees"
            } else if self.mode == Mode::Worktree {
                "Working copies"
            } else if self.mode == Mode::Base {
                "Worktree base branch"
            } else {
                "Branches"
            })
            .key_context("GitPicker")
            .w_full()
            .when(!existing_only, |d| d.h_full())
            .when(existing_only, |d| d.max_h(px(320.)))
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .capture_action(cx.listener(|this, _: &input::Left, window, cx| {
                if this.existing_only && this.search.read(cx).content.is_empty() {
                    window.dispatch_action(Box::new(popover::CloseSubmenu), cx);
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &input::Up, _, cx| {
                this.step(-1, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &input::Down, _, cx| {
                this.step(1, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &input::Submit, _, cx| {
                this.pick(this.active, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|_, _: &popover::Cancel, _, cx| {
                cx.emit(Picked::Dismissed);
                cx.stop_propagation();
            }))
            .on_key_down(cx.listener(|_, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.emit(Picked::Dismissed);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .flex_shrink_0()
                    .h(px(36.))
                    .px(px(12.))
                    .border_b_1()
                    .border_color(content(0.07))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(icon("search", 14.).text_color(content(0.4)))
                    .child(div().flex_1().min_w_0().child(self.search.clone())),
            )
            .child(
                div()
                    .id("git-choices")
                    .role(Role::ListBox)
                    .aria_label("Working copy options")
                    .when(!existing_only, |d| d.flex_1())
                    .when(existing_only, |d| d.max_h(px(284.)))
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(4.))
                    .flex()
                    .flex_col()
                    .children(rows)
                    .when(empty, |d| {
                        d.child(
                            div()
                                .p(px(12.))
                                .text_size(px(12.))
                                .text_color(content(0.45))
                                .child(if self.state.is_none() && self.error.is_none() {
                                    "Loading…"
                                } else if existing_only
                                    && self.search.read(cx).content.trim().is_empty()
                                {
                                    "No existing worktrees"
                                } else {
                                    "No matches"
                                }),
                        )
                    }),
            )
            .when(self.mode == Mode::Branch, |d| {
                d.child(
                    div()
                        .px(px(12.))
                        .py(px(6.))
                        .text_size(px(10.))
                        .text_color(content(0.4))
                        .child("Type a new name to create a branch"),
                )
            })
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    div()
                        .debug_selector(|| "git-picker-error".into())
                        .p(px(10.))
                        .text_size(px(11.))
                        .text_color(gpui::rgb(0xf87171))
                        .child(error),
                )
            })
    }
}

pub(crate) fn base_label(base: &str) -> &str {
    base.strip_prefix("refs/heads/")
        .or_else(|| base.strip_prefix("refs/remotes/"))
        .unwrap_or(base)
}
