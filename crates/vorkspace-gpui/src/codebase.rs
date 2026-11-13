//! Native Codebase page; the core service is shared by windows and app CLI.
use crate::{
    controls::page,
    input::{self, Input},
    markdown::Document,
};
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, Global, ScrollHandle, Subscription, Task, Window,
    div, prelude::*, px,
};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Duration};
use vorkspace_core::codebase::{Service, Snapshot};
use vorkspace_runtime::runtime::{Database, locked};
use vorkspace_ui::theme::*;

pub(crate) struct Shared(pub Entity<Owner>);
impl Global for Shared {}
pub(crate) struct Owner {
    pub service: Result<Arc<Service>, String>,
    _updates: Option<Task<()>>,
}
pub(crate) fn get(cx: &App) -> Option<Entity<Owner>> {
    cx.try_global::<Shared>().map(|s| s.0.clone())
}
pub(crate) fn service(cx: &App) -> Result<Arc<Service>, String> {
    get(cx)
        .ok_or("Codebase Memory is unavailable")?
        .read(cx)
        .service
        .clone()
}
pub(crate) fn enabled(project: &str, cx: &App) -> bool {
    service(cx).is_ok_and(|s| s.enabled(project))
}
pub(crate) fn action(
    service: &Service,
    project: &str,
    root: &str,
    action: &str,
    input: &Value,
) -> Result<Value, String> {
    match action {
        "codebase.tools" => Ok(vorkspace_core::codebase::tools()),
        "codebase.status" => {
            serde_json::to_value(service.snapshot(project, root)).map_err(|e| e.to_string())
        }
        "codebase.configure" => {
            let enabled = input["enabled"].as_bool().ok_or("Provide enabled")?;
            service.configure(project, enabled)?;
            if enabled {
                service.activate(project, root, false)?;
            }
            serde_json::to_value(service.snapshot(project, root)).map_err(|e| e.to_string())
        }
        "codebase.index" => {
            service.activate(project, root, true)?;
            Ok(json!({"started":true,"root":root}))
        }
        "codebase.query" => service
            .query(
                project,
                root,
                input["tool"].as_str().ok_or("Provide tool")?,
                input.get("arguments").cloned().unwrap_or(json!({})),
            )?
            .recv_blocking()
            .map_err(|e| e.to_string())?,
        _ => Err("Unknown Codebase action".into()),
    }
}
pub(crate) fn install(store: Database, cx: &mut App) {
    if get(cx).is_some() {
        return;
    }
    let root = locked(&store)
        .ok()
        .and_then(|s| s.data_dir().map(PathBuf::from));
    let result = root
        .ok_or_else(|| "Codebase Memory needs a local app database".to_owned())
        .and_then(|root| Service::new(store, root.join("codebase")));
    let owner = cx.new(|cx| match result {
        Ok((service, updates)) => {
            let task = cx.spawn(async move |this, cx| {
                while updates.recv().await.is_ok() {
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            });
            Owner {
                service: Ok(service),
                _updates: Some(task),
            }
        }
        Err(error) => Owner {
            service: Err(error),
            _updates: None,
        },
    });
    let quit = owner.read(cx).service.clone();
    cx.on_app_quit(move |cx| {
        let service = quit.clone();
        cx.background_executor().spawn(async move {
            if let Ok(service) = service {
                service.shutdown();
            }
        })
    })
    .detach();
    cx.set_global(Shared(owner));
}

#[derive(Clone)]
struct Symbol {
    name: String,
    qualified: String,
    kind: String,
    file: String,
    lines: String,
}
fn symbols(value: &Value) -> Vec<Symbol> {
    // Full-text search returns ranked rows with complete qualified names.
    // Browsing the index returns rows grouped by their qualified-name prefix.
    if let (Some(columns), Some(rows)) = (value["cols"].as_array(), value["rows"].as_array()) {
        let column = |name| {
            columns
                .iter()
                .position(|column| column.as_str() == Some(name))
        };
        if let Some(qualified) = column("qn") {
            let kind = column("label");
            let file = column("file");
            let lines = column("lines");
            return rows
                .iter()
                .filter_map(|row| {
                    let qualified = row[qualified].as_str()?;
                    let field = |index: Option<usize>| {
                        index
                            .and_then(|index| row[index].as_str())
                            .unwrap_or_default()
                    };
                    Some(Symbol {
                        name: qualified.rsplit('.').next().unwrap_or(qualified).into(),
                        qualified: qualified.into(),
                        kind: field(kind).into(),
                        file: field(file).into(),
                        lines: field(lines).into(),
                    })
                })
                .take(200)
                .collect();
        }
    }
    value["groups"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|group| {
            let prefix = group["qn_prefix"].as_str().unwrap_or("");
            group["rows"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(move |row| {
                    let name = row[0].as_str()?;
                    Some(Symbol {
                        name: name.into(),
                        qualified: if prefix.is_empty() {
                            name.into()
                        } else {
                            format!("{prefix}.{name}")
                        },
                        kind: row[1].as_str().unwrap_or("").into(),
                        file: group["file"].as_str().unwrap_or("").into(),
                        lines: row[2].as_str().unwrap_or("").into(),
                    })
                })
        })
        .take(200)
        .collect()
}

#[test]
fn ranked_codebase_results_keep_the_identity_used_to_open_source() {
    let response = json!({
        "cols": ["qn", "label", "file", "lines", "rank"],
        "rows": [
            ["atlas.src.greet.greet", "Function", "src/greet.js", "1-3", -41.1],
            ["atlas.src.greet", "Module", "src/greet.js", "1-6", -1.0]
        ],
        "search_mode": "bm25"
    });
    let matches = symbols(&response);
    assert_eq!(matches.len(), 2);
    assert_eq!(matches[0].name, "greet");
    assert_eq!(matches[0].qualified, "atlas.src.greet.greet");
    assert_eq!(matches[0].kind, "Function");
    assert_eq!(matches[0].file, "src/greet.js");
    assert_eq!(matches[0].lines, "1-3");
    assert_eq!(matches[1].qualified, "atlas.src.greet");
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SearchStatus {
    Unloaded,
    Searching,
    Complete,
}

pub(crate) struct Page {
    project: String,
    root: String,
    service: Result<Arc<Service>, String>,
    snapshot: Snapshot,
    query: Entity<Input>,
    focus: FocusHandle,
    symbols: Vec<Symbol>,
    selected: Option<Symbol>,
    source: Option<Entity<Document>>,
    relationships: Vec<(String, bool)>,
    activating: bool,
    search_status: SearchStatus,
    detail_loading: bool,
    more: bool,
    error: Option<String>,
    detail_error: Option<String>,
    search_epoch: u64,
    detail_epoch: u64,
    width: f32,
    show_detail: bool,
    list_scroll: ScrollHandle,
    detail_scroll: ScrollHandle,
    debounce: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}
impl Focusable for Page {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Page {
    pub(crate) fn new(project: String, root: String, cx: &mut Context<Self>) -> Self {
        let service = service(cx);
        let query = cx.new(|cx| {
            Input::field(
                "Search code symbols",
                "Find a symbol…",
                page::secondary_ink(),
                cx,
            )
        });
        let mut subscriptions =
            vec![
                cx.subscribe(&query, |this: &mut Self, _, _: &input::Changed, cx| {
                    this.search_epoch += 1;
                    this.debounce = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(250))
                            .await;
                        let _ = this.update(cx, |this, cx| this.search(cx));
                    }));
                }),
            ];
        if let Some(owner) = get(cx) {
            subscriptions.push(cx.observe(&owner, |this, _, cx| this.refresh(cx)));
        }
        let snapshot = service
            .as_ref()
            .map(|s| s.snapshot(&project, &root))
            .unwrap_or_default();
        let focus = cx.focus_handle();
        let mut page = Self {
            project,
            root,
            service,
            snapshot,
            query,
            focus,
            symbols: Vec::new(),
            selected: None,
            source: None,
            relationships: Vec::new(),
            activating: false,
            search_status: SearchStatus::Unloaded,
            detail_loading: false,
            more: false,
            error: None,
            detail_error: None,
            search_epoch: 0,
            detail_epoch: 0,
            width: 1000.,
            show_detail: false,
            list_scroll: ScrollHandle::new(),
            detail_scroll: ScrollHandle::new(),
            debounce: None,
            _subscriptions: subscriptions,
        };
        if page.snapshot.enabled {
            page.activate(false, cx);
        }
        page
    }
    pub(crate) fn set_width(&mut self, width: f32, cx: &mut Context<Self>) {
        if self.width != width {
            self.width = width;
            cx.notify();
        }
    }
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Ok(service) = &self.service else {
            return;
        };
        let next = service.snapshot(&self.project, &self.root);
        if next == self.snapshot {
            return;
        }
        let ready = next.enabled && next.phase == "ready" && self.snapshot.phase != "ready";
        if !next.enabled || next.phase != "ready" {
            self.search_epoch += 1;
            self.detail_epoch += 1;
            self.source = None;
            self.selected = None;
            self.symbols.clear();
            self.relationships.clear();
            self.search_status = SearchStatus::Unloaded;
            self.detail_loading = false;
            self.more = false;
            self.show_detail = false;
            self.detail_error = None;
            self.debounce = None;
        }
        self.snapshot = next;
        if ready {
            self.search(cx);
        }
        cx.notify();
    }
    pub(crate) fn activate(&mut self, reindex: bool, cx: &mut Context<Self>) {
        if self.activating
            || (reindex && matches!(self.snapshot.phase.as_str(), "preparing" | "indexing"))
        {
            return;
        }
        let Ok(service) = self.service.clone() else {
            return;
        };
        if !service.enabled(&self.project) {
            self.refresh(cx);
            return;
        }
        self.activating = true;
        let (project, root) = (self.project.clone(), self.root.clone());
        let work = cx
            .background_executor()
            .spawn(async move { service.activate(&project, &root, reindex) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.activating = false;
                this.refresh(cx);
                if result.is_ok()
                    && this.search_status == SearchStatus::Unloaded
                    && this.debounce.is_none()
                    && this.snapshot.phase == "ready"
                {
                    this.search(cx);
                }
                this.error = result.err();
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn search(&mut self, cx: &mut Context<Self>) {
        self.debounce = None;
        if self.snapshot.phase != "ready" {
            return;
        }
        let Ok(service) = self.service.clone() else {
            return;
        };
        self.search_epoch += 1;
        let epoch = self.search_epoch;
        let query = self.query.read(cx).content.to_string();
        let arguments = if query.trim().is_empty() {
            json!({"limit":100,"format":"json"})
        } else {
            json!({"query":query,"limit":100,"format":"json"})
        };
        let reply = service.query(&self.project, &self.root, "search_graph", arguments);
        self.search_status = SearchStatus::Searching;
        self.error = None;
        cx.spawn(async move |this, cx| {
            let result = match reply {
                Ok(reply) => reply
                    .recv()
                    .await
                    .unwrap_or_else(|_| Err("Codebase connection closed".into())),
                Err(e) => Err(e),
            };
            let _ = this.update(cx, |this, cx| {
                if this.search_epoch != epoch
                    || !this
                        .service
                        .as_ref()
                        .is_ok_and(|s| s.enabled(&this.project))
                {
                    return;
                }
                match result {
                    Ok(value) => {
                        this.search_status = SearchStatus::Complete;
                        this.symbols = symbols(&value);
                        this.more = value["has_more"].as_bool().unwrap_or(false);
                        this.list_scroll.scroll_to_item(0);
                    }
                    Err(error) => {
                        this.search_status = SearchStatus::Unloaded;
                        this.error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn select(&mut self, symbol: Symbol, cx: &mut Context<Self>) {
        let Ok(service) = self.service.clone() else {
            return;
        };
        self.detail_epoch += 1;
        let epoch = self.detail_epoch;
        let (project, root, qualified) = (
            self.project.clone(),
            self.root.clone(),
            symbol.qualified.clone(),
        );
        self.selected = Some(symbol);
        self.source = None;
        self.relationships.clear();
        self.show_detail = true;
        self.detail_error = None;
        self.detail_loading = true;
        cx.spawn(async move |this, cx| {
            let source = async {
                service
                    .query(
                        &project,
                        &root,
                        "get_code_snippet",
                        json!({
                            "qualified_name": qualified, "max_lines": 200,
                            "source_mode": "full", "format": "json"
                        }),
                    )?
                    .recv()
                    .await
                    .map_err(|e| e.to_string())?
            }
            .await;
            let shown = this
                .update(cx, |this, cx| {
                    if this.detail_epoch != epoch
                        || !this
                            .service
                            .as_ref()
                            .is_ok_and(|s| s.enabled(&this.project))
                    {
                        return false;
                    }
                    this.detail_loading = false;
                    match source {
                        Ok(source) => {
                            if let Some(selected) = &mut this.selected {
                                let file = source["file_path"].as_str().unwrap_or("");
                                selected.file = file
                                    .strip_prefix(&format!("{}/", this.snapshot.root))
                                    .unwrap_or(file)
                                    .into();
                                selected.lines =
                                    format!("{}–{}", source["start_line"], source["end_line"]);
                            }
                            let text = source["source"].as_str().unwrap_or("Source unavailable");
                            let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
                            let fence = "`".repeat(longest.max(2) + 1);
                            let source = format!("{fence}\n{text}\n{fence}");
                            this.source = Some(
                                cx.new(|cx| Document::prose(&source, "Source unavailable", cx)),
                            );
                            this.detail_scroll.scroll_to_item(0);
                        }
                        Err(error) => this.detail_error = Some(error),
                    }
                    cx.notify();
                    this.source.is_some()
                })
                .unwrap_or(false);
            if !shown {
                return;
            }
            // Source is already readable. Only the current selection requests links.
            let links = async {
                service
                    .query(
                        &project,
                        &root,
                        "trace_path",
                        json!({
                            "function_name": qualified, "direction": "both",
                            "depth": 1, "limit": 50, "format": "json"
                        }),
                    )?
                    .recv()
                    .await
                    .map_err(|e| e.to_string())?
            }
            .await;
            let _ = this.update(cx, |this, cx| {
                if this.detail_epoch != epoch || !service.enabled(&project) {
                    return;
                }
                match links {
                    Ok(links) => this.relationships = links_list(&links),
                    Err(error) => {
                        this.detail_error = Some(format!("Relationships unavailable: {error}"))
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

fn links_list(links: &Value) -> Vec<(String, bool)> {
    ["callers", "callees"]
        .into_iter()
        .flat_map(|side| {
            links[side]["groups"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(move |group| {
                    let prefix = group["qn_prefix"].as_str().unwrap_or("");
                    group["rows"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(move |row| {
                            row[0].as_str().map(|name| {
                                (
                                    if prefix.is_empty() {
                                        name.into()
                                    } else {
                                        format!("{prefix}.{name}")
                                    },
                                    side == "callers",
                                )
                            })
                        })
                })
        })
        .take(50)
        .collect()
}
fn graph(selected: &str, links: &[(String, bool)]) -> String {
    let label = |s: &str| {
        s.replace('&', "&amp;")
            .replace('"', "&quot;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace(['\n', '\r'], " ")
    };
    let mut graph = format!("flowchart LR\n n0[\"{}\"]\n", label(selected));
    for (index, (name, caller)) in links.iter().take(12).enumerate() {
        let id = index + 1;
        graph.push_str(&format!(
            " n{id}[\"{}\"]\n",
            label(name.rsplit('.').next().unwrap_or(name))
        ));
        graph.push_str(&if *caller {
            format!(" n{id} --> n0\n")
        } else {
            format!(" n0 --> n{id}\n")
        });
    }
    graph
}

impl Page {
    fn list(&self, narrow: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        div()
            .w(if narrow {
                px(self.width)
            } else {
                px(page::LIST_WIDTH)
            })
            .min_w_0()
            .flex_shrink_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                page::toolbar().child(
                    page::search_frame(&self.query.read(cx).focus_handle(cx))
                        .flex_1()
                        .child(icon("search", 14.))
                        .child(self.query.clone()),
                ),
            )
            .child(
                div()
                    .id("codebase-symbols")
                    .debug_selector(|| "codebase-symbols".into())
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.list_scroll)
                    .p_2()
                    .when(self.search_status == SearchStatus::Searching, |d| {
                        d.child(page::meta().p_2().child("Searching…"))
                    })
                    .when(
                        self.search_status == SearchStatus::Complete && self.symbols.is_empty(),
                        |d| d.child(page::meta().p_2().child("No matching symbols")),
                    )
                    .children(self.symbols.iter().enumerate().map(|(index, symbol)| {
                        let symbol = symbol.clone();
                        page::row(
                            ("codebase-symbol", index),
                            symbol.name.clone(),
                            self.selected
                                .as_ref()
                                .is_some_and(|s| s.qualified == symbol.qualified),
                        )
                        .w_full()
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap_1()
                        .child(page::label().w_full().truncate().child(symbol.name.clone()))
                        .child(
                            page::meta()
                                .w_full()
                                .truncate()
                                .child(format!("{} · {}", symbol.kind, symbol.file)),
                        )
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.select(symbol.clone(), cx)),
                        )
                    }))
                    .when(self.more, |d| {
                        d.child(
                            page::meta()
                                .p_2()
                                .child("First 100 matches. Refine your search to find more."),
                        )
                    }),
            )
            .into_any_element()
    }

    fn detail(&self, narrow: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        page::detail_pane(!narrow)
            .child(
                div()
                    .id("codebase-detail")
                    .debug_selector(|| "codebase-detail".into())
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.detail_scroll)
                    .p_4()
                    .when(narrow, |d| {
                        d.child(
                            page::action("codebase-back", "Back to symbols", true)
                                .debug_selector(|| "codebase-back".into())
                                .child("Back to symbols")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.show_detail = false;
                                    cx.notify();
                                })),
                        )
                    })
                    .when_some(self.selected.clone(), |d, s| {
                        d.child(page::title().child(s.name)).child(
                            page::meta()
                                .mb_4()
                                .child(format!("{} · lines {}", s.file, s.lines)),
                        )
                    })
                    .when(self.selected.is_none(), |d| {
                        d.child(page::empty(
                            icon("network", 24.),
                            "Explore this codebase",
                            "Find a symbol to inspect its source and direct callers and callees.",
                        ))
                    })
                    .when(self.detail_loading, |d| {
                        d.child(page::meta().child("Loading source…"))
                    })
                    .when_some(self.source.clone(), |d, source| d.child(source))
                    .when_some(self.detail_error.clone(), |d, error| {
                        d.child(page::meta().child(error))
                    })
                    .when(!self.relationships.is_empty(), |d| {
                        let selected = self
                            .selected
                            .as_ref()
                            .map(|s| s.name.as_str())
                            .unwrap_or("");
                        d.child(page::label().mt_6().mb_2().child("Direct relationships"))
                            .child(crate::diagrams::diagram(
                                crate::diagrams::DiagramKind::Mermaid,
                                graph(selected, &self.relationships).into(),
                                Some("Direct calls".into()),
                                || {
                                    page::meta()
                                        .child("Rendering relationships…")
                                        .into_any_element()
                                },
                            ))
                            .when(self.relationships.len() > 12, |d| {
                                d.child(
                                    page::meta().child("Diagram shows the first 12 relationships."),
                                )
                            })
                            .children(self.relationships.iter().enumerate().map(
                                |(index, (name, caller))| {
                                    let name = name.clone();
                                    let short = name.rsplit('.').next().unwrap_or(&name).to_owned();
                                    page::action(("codebase-link", index), name.clone(), true)
                                        .w_full()
                                        .justify_start()
                                        .child(icon(
                                            if *caller { "arrow-down" } else { "arrow-up" },
                                            14.,
                                        ))
                                        .child(page::meta().child(if *caller {
                                            "Called by"
                                        } else {
                                            "Calls"
                                        }))
                                        .child(div().truncate().child(name.clone()))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.select(
                                                Symbol {
                                                    name: short.clone(),
                                                    qualified: name.clone(),
                                                    kind: String::new(),
                                                    file: String::new(),
                                                    lines: String::new(),
                                                },
                                                cx,
                                            )
                                        }))
                                },
                            ))
                    }),
            )
            .into_any_element()
    }
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = self.snapshot.phase == "ready";
        let narrow = self.width < 740.;
        let status = match self.snapshot.phase.as_str() {
            "disabled" => "Enable Codebase Memory in this project's settings.",
            "preparing" => "Preparing the local engine…",
            "indexing" => "Indexing this working copy…",
            "ready"
                if self
                    .snapshot
                    .index
                    .as_ref()
                    .is_some_and(|v| v["watch"]["watched"] == true) =>
            {
                "Local index · watches for file changes"
            }
            "ready" => "Local index · reindex to refresh",
            "error" => "Index unavailable",
            _ => "Index this working copy to explore its code.",
        };
        let index = self.snapshot.index.as_ref();
        let count = |key: &str| index.and_then(|v| v[key].as_u64()).unwrap_or(0);
        let incomplete = index
            .map(|v| {
                ["skipped", "parse_partial", "parse_unusable"]
                    .iter()
                    .map(|key| v[key]["count"].as_u64().unwrap_or(0))
                    .sum::<u64>()
                    + v["not_indexed"]["files_count"].as_u64().unwrap_or(0)
            })
            .unwrap_or(0);
        let error = self
            .service
            .as_ref()
            .err()
            .or(self.error.as_ref())
            .or(self.snapshot.error.as_ref())
            .cloned();
        div()
            .size_full()
            .min_h_0()
            .text_color(content(1.))
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .child(
                page::toolbar()
                    .child(page::label().child("Codebase"))
                    .child(
                        page::meta()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(self.root.clone()),
                    )
                    .child(
                        page::action(
                            "codebase-reindex",
                            if ready { "Reindex" } else { "Retry" },
                            self.snapshot.enabled
                                && !self.activating
                                && !matches!(
                                    self.snapshot.phase.as_str(),
                                    "preparing" | "indexing"
                                ),
                        )
                        .child(if ready { "Reindex" } else { "Retry" })
                        .on_click(cx.listener(|this, _, _, cx| this.activate(true, cx))),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .py_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(page::meta().child(status))
                    .when(ready, |d| {
                        d.child(page::meta().child(format!(
                            "{} symbols · {} relationships",
                            count("nodes"),
                            count("edges")
                        )))
                    })
                    .when(incomplete > 0, |d| {
                        d.child(page::meta().child(format!(
                            "Partial coverage: {incomplete} skipped, partial, or unindexed files."
                        )))
                    })
                    .when_some(error, |d, e| {
                        d.child(
                            page::meta()
                                .text_color(vorkspace_ui::theme::status(StatusColor::Danger, 1.))
                                .child(e),
                        )
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .when(!narrow || !self.show_detail, |d| {
                        d.child(self.list(narrow, cx))
                    })
                    .when(!narrow || self.show_detail, |d| {
                        d.child(self.detail(narrow, cx))
                    }),
            )
    }
}

#[cfg(test)]
mod tests;
