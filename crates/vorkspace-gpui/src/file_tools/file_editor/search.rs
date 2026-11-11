//! editorSearch.ts's two-row Find/Replace panel, using the same query engine.
use super::*;
use vorkspace_editor::editor_analysis::{self, SearchMatch};
use vorkspace_ui::radius;

#[derive(Clone, PartialEq, Eq)]
struct Request {
    source: SharedString,
    query: SharedString,
    replacement: SharedString,
    case: bool,
    word: bool,
    regex: bool,
}

pub(super) struct Matches {
    request: Request,
    rows: Vec<SearchMatch>,
    index: usize,
}

#[derive(Default)]
pub(super) enum FindState {
    #[default]
    Empty,
    Searching {
        _task: Task<()>,
        anchor: usize,
    },
    Ready(Matches),
    InvalidPattern,
    Failed,
}

impl FindState {
    fn anchor(&self) -> usize {
        match self {
            Self::Searching { anchor, .. } => *anchor,
            Self::Ready(found) => found.rows.get(found.index).map_or(0, |row| row.from),
            _ => 0,
        }
    }

    pub(super) fn count(&self) -> usize {
        match self {
            Self::Ready(found) => found.rows.len(),
            _ => 0,
        }
    }
}

pub(super) enum FindIntent {
    SelectMatch,
    PreserveSelection,
}

pub(super) enum SearchOption {
    Case,
    Word,
    Regex,
}

impl FileEditor {
    pub(super) fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.document.is_none() {
            return;
        }
        self.preview = false;
        if !self.search_open {
            self.search_open = true;
            let input = self.input.read(cx);
            let selected = input.content[input.selection_range()].to_owned();
            if !selected.is_empty() && !selected.contains('\n') {
                self.search.update(cx, |i, cx| i.set_text(selected, cx));
            } else {
                self.find_matches(FindIntent::SelectMatch, cx);
            }
        }
        window.focus(&self.search.focus_handle(cx), cx);
        cx.notify();
    }
    fn find_request(&self, cx: &App) -> Request {
        Request {
            source: self.input.read(cx).content.clone(),
            query: self.search.read(cx).content.clone(),
            replacement: self.replacement.read(cx).content.clone(),
            case: self.match_case,
            word: self.whole_word,
            regex: self.regex,
        }
    }

    pub(super) fn toggle_search_option(&mut self, option: SearchOption, cx: &mut Context<Self>) {
        if !self.search_open {
            cx.propagate();
            return;
        }
        let enabled = match option {
            SearchOption::Case => &mut self.match_case,
            SearchOption::Word => &mut self.whole_word,
            SearchOption::Regex => &mut self.regex,
        };
        *enabled = !*enabled;
        self.find_matches(FindIntent::SelectMatch, cx);
    }

    pub(super) fn find_matches(&mut self, intent: FindIntent, cx: &mut Context<Self>) {
        if !self.search_open {
            return;
        }
        let request = self.find_request(cx);
        let anchor = self.find_state.anchor();
        self.input
            .update(cx, |i, cx| i.set_search_matches(Vec::new(), cx));
        if request.query.is_empty() {
            self.find_state = FindState::Empty;
            cx.notify();
            return;
        }
        let selection = matches!(intent, FindIntent::SelectMatch)
            .then(|| self.input.read(cx).selection_range());
        let task = cx.spawn(async move |this, cx| {
            let work = request.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    editor_analysis::search(
                        &work.source,
                        &work.query,
                        &work.replacement,
                        work.case,
                        work.word,
                        work.regex,
                    )
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if !this.search_open || this.find_request(cx) != request {
                    return;
                }
                this.find_state = match result {
                    Ok(result) if result.invalid => FindState::InvalidPattern,
                    Ok(result) => {
                        let rows = result.matches;
                        let index = rows.iter().position(|m| m.from >= anchor).unwrap_or(0);
                        this.input.update(cx, |input, cx| {
                            input.set_search_matches(
                                rows.iter().map(|m| m.from..m.to).collect(),
                                cx,
                            );
                            if selection.is_some_and(|old| old == input.selection_range())
                                && let Some(m) = rows.get(index)
                            {
                                input.select_range(m.from..m.to, cx);
                            }
                        });
                        FindState::Ready(Matches {
                            request,
                            rows,
                            index,
                        })
                    }
                    Err(_) => FindState::Failed,
                };
                cx.notify();
            });
        });
        self.find_state = FindState::Searching {
            _task: task,
            anchor,
        };
        cx.notify();
    }
    pub(super) fn step_match(&mut self, backwards: bool, cx: &mut Context<Self>) {
        let request = self.find_request(cx);
        let FindState::Ready(found) = &mut self.find_state else {
            return;
        };
        if found.request != request || found.rows.is_empty() {
            return;
        }
        found.index = (found.index as isize + if backwards { -1 } else { 1 })
            .rem_euclid(found.rows.len() as isize) as usize;
        let m = &found.rows[found.index];
        self.input
            .update(cx, |i, cx| i.select_range(m.from..m.to, cx));
        cx.notify();
    }
    pub(super) fn replace_matches(&mut self, all: bool, cx: &mut Context<Self>) {
        let FindState::Ready(found) = &self.find_state else {
            return;
        };
        if !self.search_open
            || self.input.read(cx).is_composing()
            || found.request != self.find_request(cx)
        {
            return;
        }
        let Some(current) = found.rows.get(found.index) else {
            return;
        };
        let mut text = found.request.source.to_string();
        let cursor = if all {
            for m in found.rows.iter().rev() {
                text.replace_range(m.from..m.to, &m.insert);
            }
            0
        } else {
            text.replace_range(current.from..current.to, &current.insert);
            current.from + current.insert.len()
        };
        self.input.update(cx, |i, cx| {
            i.replace_bytes(0..i.content.len(), &text, cursor, cx)
        });
    }
    pub(super) fn dismiss_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_open {
            self.search_open = false;
            if matches!(self.find_state, FindState::Searching { .. }) {
                self.find_state = FindState::Empty;
            }
            self.input
                .update(cx, |i, cx| i.set_search_matches(Vec::new(), cx));
            window.focus(&self.input.focus_handle(cx), cx);
            cx.notify();
        } else {
            cx.propagate();
        }
    }

    pub(super) fn search_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let (status, bad) = match &self.find_state {
            FindState::Empty => (String::new(), false),
            FindState::Searching { .. } => ("Searching…".into(), false),
            FindState::Ready(found) if found.rows.is_empty() => ("No results".into(), true),
            FindState::Ready(found) => (
                format!("{} of {}", found.index + 1, found.rows.len()),
                false,
            ),
            FindState::InvalidPattern => ("Invalid regex".into(), true),
            FindState::Failed => ("Search couldn’t finish".into(), true),
        };
        let has_matches = self.find_state.count() > 0;
        let can_replace = has_matches && !self.input.read(cx).is_composing();
        let query = div()
            .h(px(26.))
            .min_w_0()
            .flex_1()
            .flex()
            .items_center()
            .px_2()
            .rounded(px(radius::CONTROL))
            .border_1()
            .border_color(if bad {
                vorkspace_ui::theme::danger().opacity(0.55)
            } else {
                content(0.1)
            })
            .bg(content(0.06))
            .child(div().flex_1().min_w_0().child(self.search.clone()))
            .child(
                div()
                    .id("file-find-status")
                    .when(!status.is_empty(), |d| {
                        d.role(Role::Status).aria_label(status.clone())
                    })
                    .min_w(px(85.))
                    .text_right()
                    .text_size(px(11.))
                    .text_color(if bad {
                        vorkspace_ui::theme::danger()
                    } else {
                        content(0.45)
                    })
                    .child(status),
            );
        let toggle = |id, label: &'static str, title: &'static str, on| {
            div()
                .id(id)
                .role(Role::Button)
                .map(vorkspace_ui::theme::tab_reachable)
                .aria_label(title)
                .aria_toggled(if on {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(radius::CONTROL))
                .text_size(px(11.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(content(if on { 1. } else { 0.6 }))
                .when(on, |d| d.bg(accent_color().opacity(0.25)))
                .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.1)))
                .child(label)
        };
        div()
            .flex_shrink_0()
            .flex()
            .items_start()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(stroke())
            .font_family(vorkspace_ui::fonts::FIXED_FONT)
            .child(
                icon_action(
                    "toggle-replace",
                    "Toggle Replace",
                    if self.replace_open {
                        "chevron-down"
                    } else {
                        "chevron-right"
                    },
                    true,
                )
                .aria_expanded(self.replace_open)
                .mt(px(3.))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.replace_open = !this.replace_open;
                    cx.notify();
                })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .h(px(26.))
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(query)
                            .child(
                                toggle("match-case", "Aa", "Match case", self.match_case).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.toggle_search_option(SearchOption::Case, cx);
                                    }),
                                ),
                            )
                            .child(
                                toggle("whole-word", "ab", "Match whole word", self.whole_word)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_search_option(SearchOption::Word, cx);
                                    })),
                            )
                            .child(
                                toggle("use-regex", ".*", "Use regular expression", self.regex)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_search_option(SearchOption::Regex, cx);
                                    })),
                            ),
                    )
                    .when(self.replace_open, |d| {
                        d.child(
                            div()
                                .h(px(26.))
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(
                                    div()
                                        .h(px(26.))
                                        .flex_1()
                                        .min_w_0()
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .rounded(px(radius::CONTROL))
                                        .border_1()
                                        .border_color(content(0.1))
                                        .bg(content(0.06))
                                        .child(self.replacement.clone()),
                                )
                                .child(
                                    crate::controls::button("replace-next", "Replace", can_replace)
                                        .px_2()
                                        .py_1()
                                        .rounded(px(radius::CONTROL))
                                        .text_size(px(11.))
                                        .text_color(content(if can_replace { 0.6 } else { 0.3 }))
                                        .when(can_replace, |d| {
                                            d.hover(|s| s.bg(vorkspace_ui::fluid::hover(0.1)))
                                        })
                                        .child("Replace")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.replace_matches(false, cx)
                                        })),
                                )
                                .child(
                                    crate::controls::button(
                                        "replace-all",
                                        "Replace all",
                                        can_replace,
                                    )
                                    .px_2()
                                    .py_1()
                                    .rounded(px(radius::CONTROL))
                                    .text_size(px(11.))
                                    .text_color(content(if can_replace { 0.6 } else { 0.3 }))
                                    .when(can_replace, |d| {
                                        d.hover(|s| s.bg(vorkspace_ui::fluid::hover(0.1)))
                                    })
                                    .child("All")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| {
                                            this.replace_matches(true, cx)
                                        }),
                                    ),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        icon_action("find-prev", "Previous match", "chevron-up", has_matches)
                            .on_click(cx.listener(|this, _, _, cx| this.step_match(true, cx))),
                    )
                    .child(
                        icon_action("find-next", "Next match", "chevron-down", has_matches)
                            .on_click(cx.listener(|this, _, _, cx| this.step_match(false, cx))),
                    )
                    .child(
                        icon_action("close-find", "Close", "x", true)
                            .size(px(24.))
                            .on_click(cx.listener(|this, _, w, cx| this.dismiss_search(w, cx))),
                    ),
            )
            .into_any_element()
    }
}
