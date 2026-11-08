//! Sessions and documents in the Bot island; habit navigation lives in settings.
use super::*;
use vorkspace_ui::radius;

impl Bots {
    pub(crate) fn session_list(
        &mut self,
        bot: &Bot,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> (AnyElement, bool) {
        let filter = self.field(
            &format!("bot-session-search-{}", bot.id),
            "Search sessions…",
            String::new(),
            false,
            cx,
        );
        let query = if expanded {
            filter.read(cx).content.to_lowercase()
        } else {
            String::new()
        };
        let views: HashMap<_, _> = self
            .owner
            .session_views(cx)
            .into_iter()
            .map(|view| (view.id, view))
            .collect();
        let mut launches: Vec<_> = bot
            .launches
            .iter()
            .rev()
            .filter(|l| {
                (if expanded {
                    self.session_filters.matches(l, views.get(&l.session_id))
                } else {
                    views.contains_key(&l.session_id)
                }) && (query.is_empty()
                    || format!(
                        "{} {} {}",
                        views
                            .get(&l.session_id)
                            .map(|v| v.title.as_str())
                            .unwrap_or(&l.title),
                        views
                            .get(&l.session_id)
                            .map(|v| v.cwd.as_str())
                            .unwrap_or(&l.cwd),
                        l.harness
                    )
                    .to_lowercase()
                    .contains(&query))
            })
            .collect();
        // Live session metadata owns activity and recency. Launch order is only
        // a tie breaker, and compact browsing never inherits hidden filters.
        launches.sort_by_key(|launch| {
            let view = views.get(&launch.session_id);
            std::cmp::Reverse((
                view.is_some_and(|v| v.busy || v.needs_input),
                view.map_or(0, |v| v.updated_at),
            ))
        });
        let total = launches.len();
        if !expanded {
            let active = launches
                .iter()
                .take_while(|launch| {
                    views
                        .get(&launch.session_id)
                        .is_some_and(|v| v.busy || v.needs_input)
                })
                .count();
            launches.truncate(if active > 0 { active.min(5) } else { 3 });
        }
        let more = bot.launches.len() > launches.len() || total > 3;
        let filter_anchor = AnchorSlot::default();
        let bot_id = bot.id.clone();
        let search_bar = div()
            .h(px(36.))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .child(icon("search", 12.))
            .child(div().flex_1().min_w_0().child(filter))
            .child(
                icon_button("bot-session-filter", "Filter Bot sessions", "filter")
                    .relative()
                    .child(filter_anchor.probe())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_workspace_context(Some(&bot_id), cx);
                        this.filter_menu(filter_anchor.clone(), window, cx);
                    })),
            );
        let no_matches = launches.is_empty() && !bot.launches.is_empty();
        let list = div()
            .w_full()
            .flex()
            .flex_col()
            .when(expanded, |d| d.child(search_bar))
            .child(
                div()
                    .id("bot-session-list")
                    .debug_selector(|| "bot-session-list".into())
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .when(bot.launches.is_empty(), |d| {
                        d.child(
                            div()
                                .px_2()
                                .py_2()
                                .text_size(px(12.))
                                .text_color(content(0.4))
                                .child("No sessions yet"),
                        )
                    })
                    .when(no_matches, |d| {
                        d.child(empty_state(
                            "search-01",
                            if !expanded {
                                "No available sessions."
                            } else if query.trim().is_empty() {
                                "No sessions match these filters."
                            } else {
                                "No sessions match this search."
                            },
                        ))
                    })
                    .children(launches.into_iter().map(|launch| {
                        // The live session owner supplies status; saved launches retain identity.
                        let bot_id = bot.id.clone();
                        let id = launch.session_id;
                        let state = views.get(&id);
                        let title = state
                            .map(|s| s.title.clone())
                            .unwrap_or(launch.title.clone());
                        let cwd = state.map(|s| &s.cwd).unwrap_or(&launch.cwd);
                        let project = bot
                            .projects
                            .iter()
                            .find(|p| {
                                vorkspace_core::files::path_key(p)
                                    == vorkspace_core::files::path_key(cwd)
                            })
                            .unwrap_or(cwd);
                        let project = std::path::Path::new(project)
                            .file_name()
                            .map_or(project.clone(), |n| n.to_string_lossy().to_string());
                        let branch = state.and_then(|s| s.branch.clone());
                        let model: SharedString = state
                            .map(|s| s.model.clone())
                            .filter(|model| !model.is_empty())
                            .unwrap_or_else(|| launch.model.clone().unwrap_or_default().into());
                        let status = match state {
                            Some(s) if s.needs_input => div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_color(vorkspace_ui::theme::warning())
                                .child(
                                    icon("circle-alert", 12.)
                                        .text_color(vorkspace_ui::theme::warning()),
                                )
                                .child("Need approval")
                                .into_any_element(),
                            Some(s) if s.busy => div()
                                .text_color(vorkspace_ui::theme::accent_color())
                                .child("Working…")
                                .into_any_element(),
                            Some(s) => div()
                                .text_color(content(0.45))
                                .child(vorkspace_core::time::relative(
                                    s.updated_at,
                                    vorkspace_core::time::now_ms(),
                                ))
                                .into_any_element(),
                            None => div()
                                .text_color(content(0.45))
                                .child("Unavailable")
                                .into_any_element(),
                        };
                        let place = match &branch {
                            Some(b) => format!("{project} · {b}"),
                            None => project,
                        };
                        div()
                            .id(("bot-launch", id as usize))
                            .debug_selector(move || format!("bot-launch-{id}"))
                            .group("bot-launch")
                            .role(Role::Button)
                            .aria_label(title.clone())
                            .px_2()
                            .py_2()
                            .flex_shrink_0()
                            .rounded(px(radius::CONTROL))
                            .border_1()
                            .border_color(if state.is_some_and(|s| s.needs_input) {
                                content(0.3)
                            } else {
                                gpui::transparent_black()
                            })
                            .when(state.is_some_and(|s| s.needs_input), |d| d.bg(content(0.2)))
                            .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.05)))
                            .when(state.is_none(), |d| d.opacity(0.5))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .min_w_0()
                                    .items_center()
                                    .gap_1p5()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(px(13.))
                                            .line_height(px(18.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(content(1.))
                                            .child(title),
                                    )
                                    .child(
                                        div()
                                            .ml_auto()
                                            .pl_2()
                                            .flex_shrink_0()
                                            .text_size(px(11.))
                                            .child(status),
                                    ),
                            )
                            .child(
                                div()
                                    .mt_1()
                                    .flex()
                                    .min_w_0()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .text_size(px(11.))
                                            .text_color(content(0.45))
                                            .child(
                                                icon(
                                                    if branch.is_some() {
                                                        "git-branch"
                                                    } else {
                                                        "folder-01"
                                                    },
                                                    12.,
                                                )
                                                .text_color(content(0.45)),
                                            )
                                            .child(div().min_w_0().truncate().child(place)),
                                    )
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .flex()
                                            .items_center()
                                            .text_size(px(11.))
                                            .text_color(content(0.5))
                                            .child(
                                                div()
                                                    .max_w(px(0.))
                                                    .overflow_hidden()
                                                    .whitespace_nowrap()
                                                    .group_hover("bot-launch", |s| {
                                                        s.max_w(px(128.)).pr(px(6.))
                                                    })
                                                    .child(model),
                                            )
                                            .child(provider_icon(&launch.harness, 14.)),
                                    ),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_workspace_context(Some(&bot_id), cx);
                                this.open_session_workspace_pane(id, cx);
                                cx.stop_propagation();
                            }))
                    })),
            )
            .into_any_element();
        (list, more)
    }
    pub(crate) fn document_list(
        &self,
        bot: &Bot,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut documents: Vec<_> = bot.documents.iter().rev().collect();
        documents.sort_by_key(|doc| std::cmp::Reverse(doc.updated_at));
        if !expanded {
            documents.truncate(3);
        }
        div()
            .id("bot-document-list")
            .debug_selector(|| "bot-document-list".into())
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_px()
            .when(bot.documents.is_empty(), |d| {
                d.child(
                    div()
                        .px_2()
                        .py_2()
                        .text_size(px(12.))
                        .text_color(content(0.4))
                        .child("No documents yet"),
                )
            })
            .children(documents.into_iter().map(|document| {
                let id = document.id.clone();
                let bot_id = bot.id.clone();
                let label = if document.kind == "html" {
                    "Visual"
                } else {
                    "Document"
                };
                div()
                    .id(SharedString::from(format!("bot-document-{id}")))
                    .debug_selector({
                        let id = id.clone();
                        move || format!("bot-document-{id}")
                    })
                    .role(Role::Button)
                    .aria_label(format!("Open {} {}", label.to_lowercase(), document.title))
                    .w_full()
                    .flex_shrink_0()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2p5()
                    .rounded(px(radius::CONTROL))
                    .px_2()
                    .py_2()
                    .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.05)))
                    .child(
                        div()
                            .size(px(28.))
                            .flex_shrink_0()
                            .rounded(px(radius::CONTROL))
                            .bg(content(0.06))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                icon(
                                    if document.kind == "html" {
                                        "chart-square"
                                    } else {
                                        "file"
                                    },
                                    14.,
                                )
                                .text_color(content(0.6)),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.))
                                    .line_height(px(20.))
                                    .text_color(content(0.85))
                                    .child(document.title.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(11.))
                                    .line_height(px(16.))
                                    .text_color(content(0.4))
                                    .child(if document.summary.trim().is_empty() {
                                        label.to_string()
                                    } else {
                                        document.summary.clone()
                                    }),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_workspace_context(Some(&bot_id), cx);
                        this.open_document_workspace_pane(&id, cx);
                        cx.stop_propagation();
                    }))
            }))
            .into_any_element()
    }
    pub(crate) fn page_header(&self, title: &str, cx: &mut Context<Self>) -> Div {
        div()
            .h(px(40.))
            .flex_shrink_0()
            .min_w_0()
            .overflow_hidden()
            .border_b_1()
            .border_color(content(0.04))
            .pl_2()
            .pr(px(44.))
            .flex()
            .items_center()
            .gap_2()
            .child(
                icon_button("bot-page-back", "Back", "chevron-left")
                    .debug_selector(|| "bot-page-back".into())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.route = None;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(title.to_string()),
            )
    }
    pub(crate) fn route_page(
        &mut self,
        bot: &Bot,
        route: Route,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match route {
            Route::Habit(id) => self.habit_page(bot, Some(&id), window, cx),
            Route::NewHabit => self.habit_page(bot, None, window, cx),
        }
    }
}
