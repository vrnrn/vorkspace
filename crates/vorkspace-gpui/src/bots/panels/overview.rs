//! The Bot overview page and its session, document and to-do lists.
use super::*;

impl Bots {
    pub(crate) fn select_overview(&mut self, tab: OverviewTab, cx: &mut Context<Self>) {
        if tab != OverviewTab::Changes {
            self.pause_changes(cx);
        }
        if self.route.is_some() {
            self.save_open_file(cx);
            self.route = None;
            cx.notify();
        }
        if self.overview_tab != tab {
            self.overview_reveal =
                Some(crate::project_tabs::TabReveal::new(tab, self.overview_tab));
            self.overview_tab = tab;
            cx.notify();
        }
    }
    pub(crate) fn overview_controls(
        &self,
        expand_label: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(bot) = self.current() else {
            return div().into_any_element();
        };
        let progress = self
            .overview_reveal
            .as_ref()
            .and_then(|r| r.progress(self.overview_tab, window));
        let tabs = [
            (
                OverviewTab::Sessions,
                "Sessions",
                "message-multiple",
                bot.launches.len(),
            ),
            (
                OverviewTab::Documents,
                "Documents",
                "file",
                bot.documents.len(),
            ),
            (OverviewTab::Habits, "Habits", "clock", self.habits.len()),
            (
                OverviewTab::Changes,
                "Changes",
                "file-diff",
                self.project_changes.count(&bot.id),
            ),
            (
                OverviewTab::Todos,
                "To-dos",
                "check-circle",
                self.open_todos(&bot.id, cx),
            ),
        ];
        div()
            .id("bot-overview-tabs")
            .debug_selector(|| "bot-overview-tabs".into())
            .role(Role::TabList)
            .aria_label("Bot details")
            .h(px(TITLE_BAR_HEIGHT))
            .flex_1()
            .min_w_0()
            .mr(px(2.))
            .flex()
            .items_center()
            .gap(px(2.))
            .children(tabs.into_iter().map(|(tab, label, glyph, count)| {
                let selected = self.overview_tab == tab;
                let unfolded = match progress {
                    Some((_, p)) if selected => p,
                    Some((closing, p)) if tab == closing => 1. - p,
                    _ if selected => 1.,
                    _ => 0.,
                };
                let id = match tab {
                    OverviewTab::Sessions => "bot-overview-Sessions",
                    OverviewTab::Documents => "bot-overview-Documents",
                    OverviewTab::Habits => "bot-overview-Habits",
                    OverviewTab::Changes => "bot-overview-Changes",
                    OverviewTab::Todos => "bot-overview-Todos",
                };
                crate::project_tabs::tab_button(
                    id,
                    (label, count),
                    glyph,
                    selected,
                    tab == OverviewTab::Changes && count > 0 && !selected,
                    if expand_label { unfolded } else { 0. },
                    window,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.select_overview(tab, cx)))
            }))
            .child(crate::popover::titlebar_drag_area().flex_1().min_w_0())
            .when(self.overview_tab == OverviewTab::Habits, |d| {
                d.child(
                    icon_button("bot-new-habit", "New habit", "plus")
                        .debug_selector(|| "bot-new-habit".into())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.route = Some(Route::NewHabit);
                            this.clear_habit_fields();
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }
    pub(crate) fn overview_panel(
        &mut self,
        bot: &Bot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_changes_summary(bot, cx);
        if self.overview_tab == OverviewTab::Changes {
            self.ensure_changes(bot, window, cx);
        } else {
            self.pause_changes(cx);
        }
        let body = match self.overview_tab {
            OverviewTab::Sessions => self.session_list(bot, cx),
            OverviewTab::Habits => self.habits_list(bot, cx),
            OverviewTab::Documents => self.document_list(bot, cx),
            OverviewTab::Changes => self.changes_list(bot, cx),
            OverviewTab::Todos => self.todo_list(bot, cx),
        };
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .child(body)
            .into_any_element()
    }
    /// Open items on the Bot's board: its open board's, which shows changes
    /// at once, or else the stored count.
    pub(crate) fn open_todos(&self, id: &str, cx: &App) -> usize {
        let key = vorkspace_core::todos::TodoBoard::Bot(id.into());
        self.todo_board
            .as_ref()
            .map(|board| board.read(cx))
            .filter(|board| board.board() == Some(&key))
            .and_then(|board| board.open_count())
            .or_else(|| self.todo_counts.get(&key.key()).copied())
            .unwrap_or(0)
    }
    pub(crate) fn load_todo_counts(&mut self, cx: &mut Context<Self>) {
        self.todo_counts_generation += 1;
        let generation = self.todo_counts_generation;
        let store = self.store.clone();
        let work = cx
            .background_executor()
            .spawn(async move { locked(&store)?.open_todo_counts() });
        cx.spawn(async move |this, cx| {
            let counts = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.todo_counts_generation != generation {
                    return;
                }
                if let Ok(counts) = counts {
                    this.todo_counts = counts;
                    cx.notify();
                }
            });
        })
        .detach();
    }
    pub(super) fn todo_list(&mut self, bot: &Bot, cx: &mut Context<Self>) -> AnyElement {
        let store = self.store.clone();
        let board = self
            .todo_board
            .get_or_insert_with(|| cx.new(|cx| crate::todos::TodoBoardView::new(store, false, cx)))
            .clone();
        let key = vorkspace_core::todos::TodoBoard::Bot(bot.id.clone());
        board.update(cx, |board, cx| board.set_board(Some(key), cx));
        div().size_full().child(board).into_any_element()
    }
    pub(super) fn session_list(&mut self, bot: &Bot, cx: &mut Context<Self>) -> AnyElement {
        let filter = self.field(
            "bot-session-search",
            "Search sessions…",
            String::new(),
            false,
            cx,
        );
        let query = filter.read(cx).content.to_lowercase();
        let views = self.owner.session_views(cx);
        let launches: Vec<_> = bot
            .launches
            .iter()
            .rev()
            .filter(|l| {
                self.session_filters
                    .matches(l, views.iter().find(|v| v.id == l.session_id))
                    && format!(
                        "{} {} {}",
                        views
                            .iter()
                            .find(|v| v.id == l.session_id)
                            .map(|v| v.title.as_str())
                            .unwrap_or(&l.title),
                        views
                            .iter()
                            .find(|v| v.id == l.session_id)
                            .map(|v| v.cwd.as_str())
                            .unwrap_or(&l.cwd),
                        l.harness
                    )
                    .to_lowercase()
                    .contains(&query)
            })
            .cloned()
            .collect();
        let search_bar = div()
            .h(px(36.))
            .px_2()
            .border_b_1()
            .border_color(crate::panel_kit::stroke())
            .flex()
            .items_center()
            .gap_2()
            .child(icon("search", 12.))
            .child(div().flex_1().min_w_0().child(filter))
            .child(
                icon_button("bot-session-filter", "Filter Bot sessions", "filter")
                    .relative()
                    .child(self.session_filter_anchor.probe())
                    .on_click(cx.listener(|this, _, window, cx| this.filter_menu(window, cx))),
            );
        let no_matches = launches.is_empty() && !bot.launches.is_empty();
        div().size_full().flex().flex_col()
            .when(!bot.launches.is_empty(),|d|d.child(search_bar))
            .child(div().id("bot-session-list").debug_selector(|| "bot-session-list".into()).flex_1().min_h_0().overflow_y_scroll().px(px(6.)).py_2().flex().flex_col().gap_1()
                .when(bot.launches.is_empty(),|d|d.child(empty_state("message-multiple",&format!("Ask {} to start a session on one of your projects, like a bug fix or a review, and it shows up here.",bot.label()))))
                .when(no_matches,|d|d.child(empty_state("search-01",if query.trim().is_empty(){"No sessions match these filters."}else{"No sessions match this search."})))
                .children(launches.into_iter().map(|launch|{
                    // Sidebar.tsx's SessionCard with `modelInFooter`.
                    let id=launch.session_id;let state=views.iter().find(|v|v.id==id);
                    let title=state.map(|s|s.title.clone()).unwrap_or(launch.title.clone());
                    let cwd=state.map(|s|&s.cwd).unwrap_or(&launch.cwd);
                    let project=bot.projects.iter().find(|p|vorkspace_core::files::path_key(p)==vorkspace_core::files::path_key(cwd)).unwrap_or(cwd);
                    let project=std::path::Path::new(project).file_name().map_or(project.clone(),|n|n.to_string_lossy().to_string());
                    let branch=state.and_then(|s|s.branch.clone());
                    let model:SharedString=state.map(|s|s.model.clone()).filter(|model| !model.is_empty()).unwrap_or_else(||launch.model.clone().unwrap_or_default().into());
                    let status=match state{
                        Some(s) if s.needs_input=>div().flex().items_center().gap_1().text_color(rgb(0xfbbf24)).child(icon("circle-alert",12.).text_color(rgb(0xfbbf24))).child("Need approval").into_any_element(),
                        Some(s) if s.busy=>div().text_color(vorkspace_ui::theme::accent_color()).child("Working...").into_any_element(),
                        Some(s)=>div().text_color(content(0.45)).child(vorkspace_core::time::relative(s.updated_at,vorkspace_core::time::now_ms())).into_any_element(),
                        None=>div().text_color(content(0.45)).child("Unavailable").into_any_element(),
                    };
                    let place=match &branch{Some(b)=>format!("{project} · {b}"),None=>project};
                    div().id(("bot-launch",id as usize)).debug_selector(move||format!("bot-launch-{id}")).group("bot-launch").role(Role::Button).aria_label(title.clone())
                        .px(px(10.)).py_2().rounded_md().border_1().border_color(if state.is_some_and(|s|s.needs_input){content(0.3)}else{gpui::transparent_black()})
                        .when(state.is_some_and(|s|s.needs_input),|d|d.bg(content(0.2)))
                        .hover(|s|s.bg(content(0.05)))
                        .when(state.is_none(),|d|d.opacity(0.5))
                        .flex().flex_col()
                        .child(div().flex().min_w_0().items_center().gap_1p5()
                            .child(div().min_w_0().truncate().text_size(px(13.)).line_height(px(18.)).font_weight(FontWeight::SEMIBOLD).text_color(content(1.)).child(title))
                            .child(div().ml_auto().pl_2().flex_shrink_0().text_size(px(11.)).child(status)))
                        .child(div().mt_1().flex().min_w_0().items_center().gap_2()
                            .child(div().flex_1().min_w_0().flex().items_center().gap_1().text_size(px(11.)).text_color(content(0.45))
                                .child(icon(if branch.is_some(){"git-branch"}else{"folder-01"},12.).text_color(content(0.45)))
                                .child(div().min_w_0().truncate().child(place)))
                            .child(div().flex_shrink_0().flex().items_center().text_size(px(11.)).text_color(content(0.5))
                                .child(div().max_w(px(0.)).overflow_hidden().whitespace_nowrap().group_hover("bot-launch",|s|s.max_w(px(128.)).pr(px(6.))).child(model))
                                .child(provider_icon(&launch.harness,14.))))
                        .on_click(cx.listener(move|this,_,_,cx|{this.route=Some(Route::Session(id));cx.notify();}))
}))).into_any_element()
    }
    pub(super) fn document_list(&self, bot: &Bot, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("bot-document-list")
            .size_full()
            .overflow_y_scroll()
            .p_2()
            .flex()
            .flex_col()
            .gap_px()
            .when(bot.documents.is_empty(), |d| {
                d.child(empty_state(
                    "file",
                    &format!(
                        "Ask {} to write something up, like a plan or a summary, and it shows up here.",
                        bot.label()
                    ),
                ))
            })
            .children(bot.documents.iter().rev().map(|document| {
                // BotOverview's DocumentRow: laid out like a habit.
                let id = document.id.clone();
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
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2p5()
                    .rounded_md()
                    .px_2()
                    .py_2()
                    .hover(|s| s.bg(content(0.05)))
                    .child(
                        div()
                            .size(px(28.))
                            .flex_shrink_0()
                            .rounded_md()
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
                        this.open_document(&id, cx);
                        this.route = Some(Route::Document(id.clone()));
                        cx.notify();
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
            .border_color(content(0.1))
            .px_2()
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
        let interactive = self.route.as_ref() == Some(&route);
        match route {
            Route::Review => div()
                .size_full()
                .flex()
                .flex_col()
                .child(self.page_header("Changes", cx))
                .when_some(self.review.clone(), |d, review| {
                    d.child(div().flex_1().min_h_0().child(review))
                })
                .into_any_element(),
            Route::Session(id) => {
                let view = self.owner.ensure_session(id, cx);
                if let Some(view) = &view {
                    view.update(cx, |v, cx| v.load(cx));
                }
                let title = view
                    .as_ref()
                    .map(|v| v.read(cx).summary.title.clone())
                    .unwrap_or_else(|| "Session unavailable".into());
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(
                        self.page_header(&title, cx).child(
                            icon_button(
                                "bot-open-session",
                                "Open session in workspace",
                                "external-link",
                            )
                            .on_click(
                                cx.listener(move |this, _, w, cx| this.open_workspace(id, w, cx)),
                            ),
                        ),
                    )
                    .when_some(view, |d, view| {
                        d.child(div().flex_1().min_h_0().child(view))
                    })
                    .into_any_element()
            }
            Route::Document(id) => {
                let Some(document) = self
                    .work
                    .documents
                    .get(&id)
                    .or_else(|| {
                        bot.documents
                            .iter()
                            .find(|d| d.id == id && !bot.desktop_document_ids.contains(&id))
                    })
                    .cloned()
                else {
                    return empty(if self.work.loading_documents.contains(&id) {
                        "Loading document…"
                    } else {
                        "Document unavailable"
                    });
                };
                let view: AnyElement = if document.kind == "html"
                    && (self.settings.is_some()
                        || self.menu.is_some()
                        || self.picker.is_some()
                        || self.background.is_some())
                {
                    div().into_any_element()
                } else if document.kind == "html" {
                    let html = self
                        .html_documents
                        .entry(id.clone())
                        .or_insert_with(|| {
                            cx.new(|_| super::html::Html::new(document.body.clone()))
                        })
                        .clone();
                    html.update(cx, |html, _| html.set_interactive(interactive));
                    html.into_any_element()
                } else {
                    self.documents
                        .entry(id.clone())
                        .or_insert_with(|| {
                            cx.new(|cx| {
                                crate::transcript::TranscriptView::document(
                                    document.body.clone(),
                                    cx,
                                )
                            })
                        })
                        .clone()
                        .into_any_element()
                };
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(
                        self.page_header(&document.title, cx).child(
                            icon_button("bot-document-copy", "Copy document", "response-copy")
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                        document.body.clone(),
                                    ))
                                }),
                        ),
                    )
                    .child(div().flex_1().min_h_0().child(view))
                    .into_any_element()
            }
            Route::Habit(id) => self.habit_page(bot, Some(&id), window, cx),
            Route::NewHabit => self.habit_page(bot, None, window, cx),
        }
    }
}
