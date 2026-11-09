//! Authenticated workspace commands use the same pane owner as direct UI input.
use super::*;
use crate::workspace::{AppPane, AppView, Connector};
use serde_json::{Value, json};
use vorkspace_core::files::path_key;

impl Shell {
    fn app_pane_in_project(&self, id: i64, project: &str, cx: &App) -> bool {
        if self.workspace.tab_of(id).is_none() {
            return false;
        }
        if let Some(pane) = self.workspace.app_panes.get(&id) {
            let selected = path_key(&self.workspace.project) == path_key(project);
            return match &pane.content {
                AppPane::App(_) | AppPane::ConnectorItem(_) | AppPane::Search => {
                    selected && self.pane_in_scope(id, cx)
                }
                AppPane::Note(note) => {
                    selected
                        && locked(&self.store)
                            .ok()
                            .and_then(|store| store.get_note(note).ok().flatten())
                            .and_then(|note| note.source_cwd)
                            .is_some_and(|owner| path_key(&owner) == path_key(project))
                }
                AppPane::BotDocument { bot, document } => {
                    selected
                        && self.bots.as_ref().is_some_and(|manager| {
                            let manager = manager.read(cx);
                            manager
                                .roster
                                .get(bot)
                                .is_some_and(|entry| entry.works_on(project))
                                && manager.document_for_pane(bot, document).is_some()
                        })
                }
                AppPane::Codebase
                | AppPane::Bot(_)
                | AppPane::Terminal { .. }
                | AppPane::Review => {
                    path_key(&pane.project) == path_key(project)
                        && path_key(&pane.worktree) == path_key(self.working_copy())
                        && self.pane_in_scope(id, cx)
                }
            };
        }
        let Some(owner) = self.workspace.session_for_pane(id) else {
            return false;
        };
        self.session_summaries.contains_key(&owner)
            && path_key(&self.session_metadata(owner, cx).summary.cwd) == path_key(project)
    }

    fn app_pane_data(&self, id: i64, cx: &App) -> Value {
        let Some(pane) = self.workspace.app_panes.get(&id) else {
            if let Some(owner) = self.workspace.review_owners.get(&id) {
                return json!({"kind":"file","sessionId":owner,"title":self.pane_title(id,cx)});
            }
            return json!({"kind":"session","sessionId":id,"title":self.pane_title(id,cx)});
        };
        match &pane.content {
            AppPane::App(AppView::Connector(provider)) => {
                json!({"kind":"connector","provider":provider.id(),"title":self.pane_title(id,cx)})
            }
            AppPane::App(AppView::Notes) => json!({"kind":"notes","title":self.pane_title(id,cx)}),
            AppPane::App(AppView::Todos) => json!({"kind":"todos","title":self.pane_title(id,cx)}),
            AppPane::App(AppView::Automations) => {
                json!({"kind":"automations","title":self.pane_title(id,cx)})
            }
            AppPane::App(AppView::Inbox) => {
                json!({"kind":"connectors","title":self.pane_title(id,cx)})
            }
            AppPane::Note(note) => {
                json!({"kind":"note","noteId":note,"title":self.pane_title(id,cx)})
            }
            AppPane::ConnectorItem(item) => {
                json!({"kind":"item","provider":item.provider,"itemKey":item.key(),"title":self.pane_title(id,cx)})
            }
            AppPane::Bot(bot) => json!({"kind":"bot","bot":bot,"title":self.pane_title(id,cx)}),
            AppPane::BotDocument { bot, document } => {
                json!({"kind":"botDocument","bot":bot,"document":document,"title":self.pane_title(id,cx)})
            }
            AppPane::Terminal { id: terminal, cwd } => {
                json!({"kind":"terminal","terminalId":terminal,"cwd":cwd,"running":self.terminal_tabs.get(terminal).and_then(|view| view.read(cx).foreground_process()),"title":self.pane_title(id,cx)})
            }
            AppPane::Codebase => {
                json!({"kind":"codebase","root":pane.worktree,"title":self.pane_title(id,cx)})
            }
            AppPane::Search => json!({"kind":"search","title":self.pane_title(id,cx)}),
            AppPane::Review => json!({"kind":"file","title":self.pane_title(id,cx)}),
        }
    }

    pub(super) fn app_workspace_action(
        &mut self,
        caller: i64,
        project: &str,
        action: &str,
        input: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        if !self.app_caller_can_read(caller, project, cx)
            || (action != "workspace.list" && !self.app_caller_allowed(caller, project, cx))
        {
            return Err("The caller no longer owns this project's app capability".into());
        }
        if action == "workspace.list" {
            let tabs = self.workspace.tabs.iter().filter_map(|&tab| {
                let layout = self.workspace.layout(tab);
                let leaves = layout.leaf_ids();
                if leaves.is_empty() || !leaves.iter().all(|&id| self.app_pane_in_project(id, project, cx) && self.pane_in_scope(id, cx)) {
                    return None;
                }
                let panes = leaves.into_iter().map(|id| {
                    let mut data = self.app_pane_data(id, cx);
                    data["paneId"] = json!(id);
                    data["available"] = json!(self.workspace.app_panes.get(&id).is_none_or(|pane| self.app_pane_available(&pane.content, cx)));
                    data
                }).collect::<Vec<_>>();
                let sashes = layout.sashes().into_iter().map(|sash| json!({
                    "sashPath":sash.path,"index":sash.index,"ratio":sash.boundary,
                    "axis": if sash.dir == SplitDir::Right { "horizontal" } else { "vertical" },
                })).collect::<Vec<_>>();
                Some(json!({"tabId":tab,"focusedPaneId":self.workspace.tab_focus(tab),"layout":layout,"panes":panes,"sashes":sashes}))
            }).collect::<Vec<_>>();
            let active = self.workspace.active.filter(|&id| {
                self.app_pane_in_project(id, project, cx) && self.pane_in_scope(id, cx)
            });
            let notes = locked(&self.store)?
                .list_notes()?
                .into_iter()
                .filter(|note| {
                    note.source_cwd
                        .as_deref()
                        .is_some_and(|owner| path_key(owner) == path_key(project))
                })
                .take(201)
                .map(|note| json!({"noteId":note.id,"title":note.title}))
                .collect::<Vec<_>>();
            let notes_truncated = notes.len() > 200;
            let available_notes = notes.into_iter().take(200).collect::<Vec<_>>();
            let (available_items, items_truncated, items_loaded) = if path_key(
                &self.workspace.project,
            ) == path_key(project)
            {
                self.inbox.as_ref().map(|owner| {
                    let (items, truncated) = owner.read(cx).items_for_project(project, 200);
                    (items.into_iter().map(|item| json!({"itemKey":item.key(),"provider":item.provider,"title":item.title,"reference":item.reference()})).collect::<Vec<_>>(), truncated, true)
                }).unwrap_or_default()
            } else {
                (Vec::new(), false, false)
            };
            let (available_bots, bots_truncated, available_documents, documents_truncated) =
                if path_key(&self.workspace.project) == path_key(project)
                    && self.workspace.preferences.bots
                {
                    self.bots.as_ref().map(|manager| {
                    let manager = manager.read(cx);
                    let bots = manager.roster.bots.iter().filter(|bot| bot.works_on(project)).collect::<Vec<_>>();
                    let bots_truncated = bots.len() > 100;
                    let names = bots.iter().take(100).map(|bot| json!({"bot":bot.id,"name":bot.label()})).collect::<Vec<_>>();
                    let documents = bots.iter().flat_map(|bot| bot.documents.iter().map(move |document| json!({"bot":bot.id,"document":document.id,"title":document.title}))).take(201).collect::<Vec<_>>();
                    let documents_truncated = documents.len() > 200;
                    (names, bots_truncated, documents.into_iter().take(200).collect(), documents_truncated)
                }).unwrap_or_default()
                } else {
                    (Vec::new(), false, Vec::new(), false)
                };
            let selected_project = (path_key(&self.workspace.project) == path_key(project))
                .then_some(self.workspace.project.as_str());
            return Ok(
                json!({"project":project,"selectedProject":selected_project,"activePaneId":active,"tabs":tabs,
                "availableNotes":available_notes,"notesTruncated":notes_truncated,
                "availableItems":available_items,"itemsTruncated":items_truncated,"itemsLoaded":items_loaded,
                "availableBots":available_bots,"botsTruncated":bots_truncated,
                "availableBotDocuments":available_documents,"documentsTruncated":documents_truncated}),
            );
        }

        // Mutations may target an assigned project other than the one the user
        // is viewing. Switch through the normal navigation owner first.
        if path_key(&self.workspace.project) != path_key(project) {
            if !self
                .workspace
                .projects
                .iter()
                .any(|path| path_key(path) == path_key(project))
            {
                return Err("Project is not open in this workspace".into());
            }
            self.select_project(project.to_owned(), window, cx);
        }
        match action {
            "workspace.open" => self.app_workspace_open(project, input, window, cx),
            "workspace.focus" => {
                let id = workspace_id(input, "paneId")?;
                self.check_workspace_pane(id, project, cx)?;
                if self
                    .workspace
                    .app_panes
                    .get(&id)
                    .is_some_and(|pane| !self.app_pane_available(&pane.content, cx))
                {
                    return Err("Pane is unavailable; enable its feature before focusing it".into());
                }
                self.open(id, window, cx);
                if path_key(&self.workspace.project) != path_key(project)
                    || self.workspace.active != Some(id)
                {
                    return Err("Pane navigation changed before focus completed".into());
                }
                Ok(
                    json!({"paneId":id,"tabId":self.workspace.tab_of(id),"focused":self.workspace.active == Some(id)}),
                )
            }
            "workspace.close" => {
                let id = workspace_id(input, "paneId")?;
                self.check_workspace_pane(id, project, cx)?;
                if !self.prepare_app_pane_close(id, cx) {
                    return Err(json!({"code":"save_pending","paneId":id,"message":"The note still has unsaved changes; inspect its save status before closing"}).to_string());
                }
                let running_process =
                    match self.workspace.app_panes.get(&id).map(|pane| &pane.content) {
                        Some(AppPane::Terminal { id: terminal, .. }) => self
                            .terminal_tabs
                            .get(terminal)
                            .and_then(|view| view.read(cx).foreground_process()),
                        _ => None,
                    };
                if let Some(process) = running_process
                    && (input.get("confirmRunning").and_then(Value::as_bool) != Some(true)
                        || input.get("expectedProcess").and_then(Value::as_str)
                            != Some(process.as_str()))
                {
                    return Err(json!({"code":"confirmation_required","paneId":id,"foregroundProcess":process,"required":{"confirmRunning":true,"expectedProcess":process},"message":"Inspect the terminal process, then use a new request ID to confirm closing this exact process"}).to_string());
                }
                if !self.tab_can_close(id, cx)
                    && !self
                        .workspace
                        .layout(self.workspace.tab_of(id).unwrap())
                        .is_split()
                {
                    return Err("The last workspace tab cannot be closed".into());
                }
                self.close_pane_confirmed(id, window, cx);
                Ok(json!({"paneId":id,"closed":self.workspace.tab_of(id).is_none()}))
            }
            "workspace.move" => {
                let target = workspace_id(input, "targetPaneId")?;
                self.check_workspace_pane(target, project, cx)?;
                let target_tab = self.workspace.tab_of(target).unwrap();
                self.check_workspace_tab(target_tab, project, cx)?;
                let source = match (input.get("paneId"), input.get("tabId")) {
                    (Some(_), None) => {
                        let id = workspace_id(input, "paneId")?;
                        self.check_workspace_pane(id, project, cx)?;
                        DragSource::Pane(id)
                    }
                    (None, Some(_)) => {
                        let tab = workspace_id(input, "tabId")?;
                        self.check_workspace_tab(tab, project, cx)?;
                        DragSource::Tab(tab)
                    }
                    _ => return Err("Provide exactly one of paneId or tabId".into()),
                };
                let edge = workspace_edge(input.get("edge").and_then(Value::as_str))?;
                if source.session() == target || !self.placement_allowed(source, target, edge) {
                    return Err(PANE_LIMIT_MESSAGE.into());
                }
                let before = self.active_leaves();
                match source {
                    DragSource::Pane(id) => self.workspace.place(id, target, edge),
                    DragSource::Tab(tab) => self.workspace.place_tab(tab, target, edge),
                    _ => unreachable!("workspace.move only accepts a pane or tab"),
                }
                let focused = self.workspace.active.ok_or("Moved pane lost focus")?;
                self.history
                    .prune(&self.workspace.open_sessions(), self.workspace.active);
                self.finish_workspace_layout(before, focused, window, cx);
                Ok(json!({"paneId":focused,"tabId":self.workspace.tab_of(focused),"moved":true}))
            }
            "workspace.detach" => {
                let id = workspace_id(input, "paneId")?;
                self.check_workspace_pane(id, project, cx)?;
                let tab = self.workspace.tab_of(id).unwrap();
                if !self.workspace.layout(tab).is_split() {
                    return Err("Pane is already its own tab".into());
                }
                let to = input
                    .get("index")
                    .and_then(Value::as_u64)
                    .map(usize::try_from)
                    .transpose()
                    .map_err(|_| "index is too large")?
                    .unwrap_or(self.visible_tabs(cx).len());
                self.workspace.detach(id, self.workspace.tabs.len());
                self.reorder_title_tab(&TitleTab::Workspace(id), to, cx);
                self.pane_motion = None;
                self.fit_active_pane_width();
                self.open(id, window, cx);
                Ok(json!({"paneId":id,"tabId":self.workspace.tab_of(id),"detached":true}))
            }
            "workspace.reorder" => {
                let tab = workspace_id(input, "tabId")?;
                self.check_workspace_tab(tab, project, cx)?;
                let visible = self.visible_tabs(cx);
                let entry = TitleTab::Workspace(tab);
                if !visible.contains(&entry) {
                    return Err("Tab is not visible in the selected project".into());
                }
                let index = input["index"]
                    .as_u64()
                    .and_then(|value| usize::try_from(value).ok())
                    .ok_or("Provide index as a non-negative integer")?;
                if index > visible.len() {
                    return Err("index is outside the visible tab strip".into());
                }
                self.reorder_title_tab(&entry, index, cx);
                self.tabs_frame.update(cx, |_, cx| cx.notify());
                self.persist(cx);
                self.invalidate_chrome(cx);
                cx.notify();
                let index = self
                    .visible_tabs(cx)
                    .iter()
                    .position(|candidate| *candidate == entry)
                    .ok_or("Tab disappeared while reordering")?;
                Ok(json!({"tabId":tab,"index":index,"reordered":true}))
            }
            "workspace.resize" => {
                let tab = workspace_id(input, "tabId")?;
                self.check_workspace_tab(tab, project, cx)?;
                let parts = input["sashPath"].as_array().ok_or("Provide sashPath")?;
                if parts.len() > 32 {
                    return Err("sashPath is too long".into());
                }
                let path = parts
                    .iter()
                    .map(|part| {
                        part.as_u64()
                            .and_then(|part| usize::try_from(part).ok())
                            .ok_or("sashPath must contain non-negative child indices")
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let index = input["index"]
                    .as_u64()
                    .and_then(|value| usize::try_from(value).ok())
                    .ok_or("Provide index as a non-negative integer")?;
                let ratio = input["ratio"]
                    .as_f64()
                    .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
                    .ok_or("Provide ratio between 0 and 1")? as f32;
                let sash = self
                    .workspace
                    .layout(tab)
                    .sashes()
                    .into_iter()
                    .find(|sash| sash.path == path && sash.index == index)
                    .ok_or("Sash is no longer in this tab")?;
                let span = if sash.dir == SplitDir::Right {
                    self.pane_available_width() * sash.group.w
                } else {
                    self.pane_bounds
                        .get()
                        .map_or(600., |bounds| bounds.size.height.as_f32())
                        * sash.group.h
                };
                let target = ResizeTarget::Sash {
                    tab,
                    path,
                    index,
                    span,
                };
                let (_, min, max, _) = self
                    .resize_extent(&target)
                    .ok_or("Sash cannot be resized")?;
                let ratio = ratio.clamp(min, max);
                self.set_resize(&target, ratio);
                self.persist(cx);
                cx.notify();
                Ok(json!({"tabId":tab,"ratio":ratio,"resized":true}))
            }
            _ => Err("Unknown workspace action".into()),
        }
    }

    fn check_workspace_pane(&self, id: i64, project: &str, cx: &App) -> Result<(), String> {
        if self.app_pane_in_project(id, project, cx) && self.pane_in_scope(id, cx) {
            if let Some(owner) = self.workspace.session_for_pane(id) {
                self.check_live_app_target(owner, cx)?;
            }
            Ok(())
        } else {
            Err("Pane is unavailable in the selected project and working copy".into())
        }
    }

    fn check_workspace_tab(&self, tab: i64, project: &str, cx: &App) -> Result<(), String> {
        if self.workspace.tab_of(tab) != Some(tab) {
            return Err("tabId must be a tab key from workspace.list".into());
        }
        let leaves = self.workspace.layout(tab).leaf_ids();
        if leaves.is_empty() {
            return Err("Tab has no panes".into());
        }
        for id in leaves {
            self.check_workspace_pane(id, project, cx)?;
        }
        Ok(())
    }

    fn finish_workspace_layout(
        &mut self,
        before: Vec<LayoutLeaf>,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.fit_active_pane_width();
        self.begin_pane_motion(before);
        self.open(id, window, cx);
    }

    fn app_workspace_open(
        &mut self,
        project: &str,
        input: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let kind = input
            .get("kind")
            .and_then(Value::as_str)
            .ok_or("Provide kind")?;
        let target = match input
            .get("placement")
            .and_then(Value::as_str)
            .unwrap_or("tab")
        {
            "tab" => None,
            "right" => Some((workspace_id(input, "targetPaneId")?, Edge::Right)),
            "down" => Some((workspace_id(input, "targetPaneId")?, Edge::Bottom)),
            _ => return Err("placement must be tab, right or down".into()),
        };

        let session = if kind == "session" {
            let value = input.get("sessionId").ok_or("Provide sessionId")?;
            let id = value
                .as_i64()
                .or_else(|| value.as_str()?.parse().ok())
                .ok_or("Provide an integer sessionId")?;
            let summary = self
                .session_summaries
                .get(&id)
                .filter(|_| self.workspace.session_for_pane(id) == Some(id))
                .ok_or("Session is unavailable")?;
            if path_key(&summary.cwd) != path_key(project) || self.deleting.contains(&id) {
                return Err("Session is outside the selected project or is being deleted".into());
            }
            self.check_live_app_target(id, cx)?;
            Some(id)
        } else {
            None
        };

        let content = match kind {
            "session" | "file" => None,
            "codebase" => {
                if !crate::codebase::enabled(project, cx) {
                    return Err("Codebase Memory is disabled for this project".into());
                }
                Some(AppPane::Codebase)
            }
            "notes" => {
                if !self.workspace.preferences.notes {
                    return Err("Notes are disabled".into());
                }
                Some(AppPane::App(AppView::Notes))
            }
            "note" => {
                if !self.workspace.preferences.notes {
                    return Err("Notes are disabled".into());
                }
                let id = workspace_text(input, "noteId", 256)?;
                let note = locked(&self.store)?
                    .get_note(&id)?
                    .ok_or("Note not found")?;
                if note
                    .source_cwd
                    .as_deref()
                    .is_none_or(|cwd| path_key(cwd) != path_key(project))
                {
                    return Err("Note is outside the selected project".into());
                }
                Some(AppPane::Note(id))
            }
            "connector" => {
                let provider = Connector::from_id(&workspace_text(input, "provider", 32)?)
                    .ok_or("Unknown connector provider")?;
                if !self.enabled_connectors(cx).contains(&provider) {
                    return Err("Connector is disabled".into());
                }
                Some(AppPane::App(AppView::Connector(provider)))
            }
            "item" => {
                let key = workspace_text(input, "itemKey", 1024)?;
                self.ensure_background_inbox(window, cx);
                let item = self.inbox.as_ref().and_then(|owner| owner.read(cx).item_by_key_for_project(&key, project))
                    .or_else(|| self.workspace.app_panes.values().find_map(|pane| {
                        match &pane.content {
                            AppPane::ConnectorItem(item) if item.key() == key
                                && ((item.provider == "jira" && item.project_path.is_empty())
                                    || (!item.project_path.is_empty() && path_key(&item.project_path) == path_key(project))) => Some((**item).clone()),
                            _ => None,
                        }
                    })).ok_or("Connector item is not in the current inbox snapshot; refresh its connector page first")?;
                let provider =
                    Connector::from_id(&item.provider).ok_or("Unknown connector provider")?;
                if !self.enabled_connectors(cx).contains(&provider) {
                    return Err("Connector is disabled".into());
                }
                Some(AppPane::ConnectorItem(Box::new(item)))
            }
            "bot" | "botDocument" => {
                let bot = workspace_text(input, "bot", 256)?;
                let manager = self.bots.as_ref().ok_or("Bots are unavailable")?;
                if !self.workspace.preferences.bots
                    || !manager
                        .read(cx)
                        .roster
                        .get(&bot)
                        .is_some_and(|entry| entry.works_on(project))
                {
                    return Err("Bot is unavailable in the selected project".into());
                }
                if kind == "bot" {
                    Some(AppPane::Bot(bot))
                } else {
                    let document = workspace_text(input, "document", 256)?;
                    if manager
                        .read(cx)
                        .document_for_pane(&bot, &document)
                        .is_none()
                    {
                        return Err("Bot document is unavailable".into());
                    }
                    Some(AppPane::BotDocument { bot, document })
                }
            }
            "terminal" => {
                let cwd = self.working_copy().to_owned();
                if cwd.starts_with("remote://")
                    || (!cfg!(test) && !std::path::Path::new(&cwd).is_dir())
                {
                    return Err("Terminal directory is unavailable".into());
                }
                None
            }
            "search" => Some(AppPane::Search),
            "todos" => {
                if !self.workspace.preferences.todos {
                    return Err("To-dos are disabled".into());
                }
                Some(AppPane::App(AppView::Todos))
            }
            "automations" => Some(AppPane::App(AppView::Automations)),
            _ => return Err("Unknown workspace pane kind".into()),
        };

        let existing = session.or_else(|| {
            content.as_ref().and_then(|content| {
                self.workspace.app_panes.iter().find_map(|(&id, pane)| {
                    (pane.content.same_resource(content)
                        && self.app_pane_in_project(id, project, cx))
                    .then_some(id)
                })
            })
        });
        if let Some(tab) = existing.and_then(|id| self.workspace.tab_of(id)) {
            // A reusable app resource can bring its entire tiled layout forward.
            // Validate every sibling before navigation changes the selected scope.
            self.check_workspace_tab(tab, project, cx)?;
        }
        let target_before = if let Some((target, edge)) = target {
            self.check_workspace_pane(target, project, cx)?;
            // A not-yet-open file uses the same exempt placeholder as a file
            // dragged from a review; it must not count as a fourth content pane.
            let source = existing.map_or_else(
                || {
                    if kind == "file" {
                        DragSource::Review(i64::MAX)
                    } else {
                        DragSource::Session(i64::MAX)
                    }
                },
                DragSource::Pane,
            );
            if existing == Some(target) || !self.placement_allowed(source, target, edge) {
                return Err(PANE_LIMIT_MESSAGE.into());
            }
            Some(
                self.workspace
                    .layout(self.workspace.tab_of(target).unwrap())
                    .leaves(),
            )
        } else {
            None
        };

        if let Some(id) = session
            && !self.in_worktree_scope(id, cx)
        {
            return Err("Session is outside the selected working copy".into());
        }

        let file_pane = if kind == "file" {
            let path = workspace_text(input, "path", 4096)?;
            let relative = std::path::Path::new(&path);
            if relative.is_absolute()
                || relative
                    .components()
                    .any(|part| !matches!(part, std::path::Component::Normal(_)))
            {
                return Err("File path must stay within the working copy".into());
            }
            let owner = target
                .map(|(target, _)| target)
                .or(self.workspace.active)
                .and_then(|pane| self.workspace.session_for_pane(pane))
                .filter(|&id| self.app_pane_in_project(id, project, cx))
                .ok_or("Open a session in this project before opening a file")?;
            self.check_live_app_target(owner, cx)?;
            let cwd = self.working_copy().to_owned();
            let root = std::path::Path::new(&cwd)
                .canonicalize()
                .map_err(|error| error.to_string())?;
            let absolute = root
                .join(relative)
                .canonicalize()
                .map_err(|error| error.to_string())?;
            if !absolute.starts_with(&root) || !absolute.is_file() {
                return Err("File path must name a file within the working copy".into());
            }
            self.open(owner, window, cx);
            self.open_review(
                crate::git_tools::review::ReviewTab::File { cwd, path },
                window,
                cx,
            );
            let pane = self
                .independent_file_pane(owner, cx)
                .ok_or("File pane could not be detached")?;
            self.open_file_pane(pane, window, cx);
            Some(pane)
        } else {
            None
        };

        if let Some(id) = session {
            self.open(id, window, cx);
        } else if kind == "terminal" {
            self.open_terminal_tab(window, cx);
        } else if let Some(content) = content.as_ref() {
            match content {
                AppPane::App(view) => self.open_title_tab(&TitleTab::App(*view), window, cx),
                AppPane::Note(id) => self.open_note_tab(id.clone(), window, cx),
                AppPane::ConnectorItem(item) => self.open_item_tab((**item).clone(), window, cx),
                AppPane::Codebase | AppPane::Bot(_) => {
                    let id = self
                        .workspace
                        .app_pane(
                            content.clone(),
                            project.to_owned(),
                            self.working_copy().to_owned(),
                        )
                        .ok_or("Could not reserve a pane ID")?;
                    self.open_app_pane(id, window, cx);
                }
                AppPane::BotDocument { .. } => {
                    let id = self
                        .workspace
                        .app_pane(
                            content.clone(),
                            project.to_owned(),
                            self.working_copy().to_owned(),
                        )
                        .ok_or("Could not reserve a pane ID")?;
                    self.open_app_pane(id, window, cx);
                }
                AppPane::Terminal { id, cwd } => {
                    self.show_terminal_tab(*id, cwd.clone(), window, cx)
                }
                AppPane::Search => self.open_app_search(window, cx),
                AppPane::Review => return Err("Choose a file to open".into()),
            }
        }
        let id = file_pane
            .or(self.workspace.active)
            .filter(|&id| {
                self.workspace.tab_of(id).is_some()
                    && (session == Some(id)
                        || file_pane == Some(id)
                        || (kind == "terminal"
                            && matches!(
                                self.workspace.app_panes.get(&id).map(|pane| &pane.content),
                                Some(AppPane::Terminal { .. })
                            ))
                        || content.as_ref().is_some_and(|content| {
                            self.workspace
                                .app_panes
                                .get(&id)
                                .is_some_and(|pane| pane.content.same_resource(content))
                        }))
            })
            .ok_or("The requested pane could not be opened")?;
        if path_key(&self.workspace.project) != path_key(project)
            || !self.app_pane_in_project(id, project, cx)
        {
            return Err(
                "The requested pane moved outside the selected project while opening".into(),
            );
        }
        if let Some((target, edge)) = target {
            self.check_workspace_pane(target, project, cx)?;
            if !self.placement_allowed(DragSource::Pane(id), target, edge) {
                return Err(
                    "The target changed while opening; the pane remains in its own tab".into(),
                );
            }
            self.workspace.place(id, target, edge);
            self.finish_workspace_layout(target_before.unwrap(), id, window, cx);
        }
        Ok(json!({"paneId":id,"tabId":self.workspace.tab_of(id),"kind":kind,"opened":true}))
    }
}

fn workspace_id(input: &Value, key: &str) -> Result<i64, String> {
    input
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("Provide {key} as an integer pane ID"))
}

fn workspace_edge(edge: Option<&str>) -> Result<Edge, String> {
    match edge {
        Some("left") => Ok(Edge::Left),
        Some("right") => Ok(Edge::Right),
        Some("top") => Ok(Edge::Top),
        Some("bottom") => Ok(Edge::Bottom),
        _ => Err("edge must be left, right, top or bottom".into()),
    }
}

fn workspace_text(input: &Value, key: &str, max: usize) -> Result<String, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty() && value.len() <= max)
        .map(str::to_owned)
        .ok_or_else(|| format!("Provide {key} (up to {max} bytes)"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn operator_reorders_tabs_and_moves_a_mixed_layout_with_ui_admission(cx: &mut TestAppContext) {
        let (window, _, caller) = super::super::tests::window(cx);
        window
            .update(cx, |shell, window, cx| {
                shell.sessions[&caller]
                    .update(cx, |session, cx| session.enable_mode("operator", cx));
                shell.open(caller, window, cx);
                let project = shell.workspace.project.clone();
                let search = shell
                    .app_workspace_action(
                        caller,
                        &project,
                        "workspace.open",
                        &json!({"kind":"search","placement":"right","targetPaneId":caller}),
                        window,
                        cx,
                    )
                    .unwrap()["paneId"]
                    .as_i64()
                    .unwrap();
                let source_tab = shell.workspace.tab_of(caller).unwrap();
                let source_layout = shell.workspace.layout(source_tab);
                assert_eq!(source_layout.leaf_ids(), [caller, search]);
                let target = shell
                    .app_workspace_action(
                        caller,
                        &project,
                        "workspace.open",
                        &json!({"kind":"automations"}),
                        window,
                        cx,
                    )
                    .unwrap()["paneId"]
                    .as_i64()
                    .unwrap();
                let target_tab = shell.workspace.tab_of(target).unwrap();
                let reordered = shell
                    .app_workspace_action(
                        caller,
                        &project,
                        "workspace.reorder",
                        &json!({"tabId":source_tab,"index":1}),
                        window,
                        cx,
                    )
                    .unwrap();
                assert_eq!(reordered["index"], 1);
                assert_eq!(
                    shell.visible_tabs(cx),
                    [
                        TitleTab::Workspace(target_tab),
                        TitleTab::Workspace(source_tab)
                    ]
                );
                assert_eq!(shell.workspace.tab_focus(source_tab), search);
                assert!(
                    shell
                        .app_workspace_action(
                            caller,
                            &project,
                            "workspace.reorder",
                            &json!({"tabId":source_tab,"index":99}),
                            window,
                            cx,
                        )
                        .is_err()
                );

                let previous_width = shell.viewport_width;
                shell.viewport_width = MIN_CHAT_WIDTH * 2.;
                assert_eq!(
                    shell
                        .app_workspace_action(
                            caller,
                            &project,
                            "workspace.move",
                            &json!({"tabId":source_tab,"targetPaneId":target,"edge":"right"}),
                            window,
                            cx,
                        )
                        .unwrap_err(),
                    PANE_LIMIT_MESSAGE
                );
                assert_eq!(shell.workspace.layout(source_tab), source_layout);
                assert_eq!(shell.workspace.tab_of(target), Some(target_tab));
                shell.viewport_width = previous_width;

                let moved = shell
                    .app_workspace_action(
                        caller,
                        &project,
                        "workspace.move",
                        &json!({"tabId":source_tab,"targetPaneId":target,"edge":"bottom"}),
                        window,
                        cx,
                    )
                    .unwrap();
                assert_eq!(moved["paneId"], search);
                assert_eq!(moved["tabId"], target_tab);
                assert_eq!(shell.workspace.tab_of(caller), Some(target_tab));
                assert_eq!(shell.workspace.tab_of(search), Some(target_tab));
                assert!(!shell.workspace.tabs.contains(&source_tab));
                assert_eq!(shell.workspace.layout(target_tab).leaf_ids().len(), 3);
                assert_eq!(shell.workspace.active, Some(search));
            })
            .unwrap();
    }

    #[gpui::test]
    fn operator_rejects_a_whole_tab_with_one_foreign_leaf(cx: &mut TestAppContext) {
        let (window, foreign, caller) = super::super::tests::window(cx);
        window
            .update(cx, |shell, window, cx| {
                shell.sessions[&caller]
                    .update(cx, |session, cx| session.enable_mode("operator", cx));
                shell.open(caller, window, cx);
                let project = shell.workspace.project.clone();
                let target = shell
                    .app_workspace_action(
                        caller,
                        &project,
                        "workspace.open",
                        &json!({"kind":"search"}),
                        window,
                        cx,
                    )
                    .unwrap()["paneId"]
                    .as_i64()
                    .unwrap();
                shell.workspace.place(foreign, caller, Edge::Bottom);
                let source_tab = shell.workspace.tab_of(caller).unwrap();
                let before = shell.workspace.layout(source_tab);
                let move_error = shell
                    .app_workspace_action(
                        caller,
                        &project,
                        "workspace.move",
                        &json!({"tabId":source_tab,"targetPaneId":target,"edge":"bottom"}),
                        window,
                        cx,
                    )
                    .unwrap_err();
                assert!(move_error.contains("unavailable"), "{move_error}");
                assert!(
                    shell
                        .app_workspace_action(
                            caller,
                            &project,
                            "workspace.reorder",
                            &json!({"tabId":source_tab,"index":0}),
                            window,
                            cx,
                        )
                        .is_err()
                );
                assert!(
                    shell
                        .app_workspace_action(
                            caller,
                            &project,
                            "workspace.resize",
                            &json!({"tabId":source_tab,"sashPath":[],"index":0,"ratio":0.5}),
                            window,
                            cx,
                        )
                        .unwrap_err()
                        .contains("unavailable")
                );
                assert_eq!(shell.workspace.layout(source_tab), before);
                assert_eq!(shell.workspace.tab_of(target), Some(target));
                shell.workspace.detach(foreign, shell.workspace.tabs.len());
                shell.open(caller, window, cx);
            })
            .unwrap();
    }

    #[gpui::test]
    fn file_admission_keeps_the_stack_exemption_and_path_boundary(cx: &mut TestAppContext) {
        let (window, _, caller) = super::super::tests::window(cx);
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("report.txt");
        std::fs::write(&file, "A file opened from an app pane").unwrap();
        window
            .update(cx, |shell, window, cx| {
                shell.sessions[&caller]
                    .update(cx, |session, cx| session.enable_mode("operator", cx));
                shell.open(caller, window, cx);
                let project = shell.workspace.project.clone();
                for kind in ["search", "automations"] {
                    shell
                        .app_workspace_action(
                            caller,
                            &project,
                            "workspace.open",
                            &json!({
                                "kind":kind,"placement":"down","targetPaneId":caller
                            }),
                            window,
                            cx,
                        )
                        .unwrap();
                }
                let tab = shell.workspace.tab_of(caller).unwrap();
                let before = shell.workspace.layout(tab);
                assert_eq!(before.leaf_ids().len(), 3);
                assert_eq!(
                    shell
                        .app_workspace_action(
                            caller,
                            &project,
                            "workspace.open",
                            &json!({
                                "kind":"notes","placement":"down","targetPaneId":caller
                            }),
                            window,
                            cx
                        )
                        .unwrap_err(),
                    PANE_LIMIT_MESSAGE
                );
                assert_eq!(shell.app_workspace_action(caller, &project, "workspace.open", &json!({
                "kind":"file","path":"../escape","placement":"down","targetPaneId":caller
            }), window, cx).unwrap_err(), "File path must stay within the working copy");
                assert_eq!(shell.workspace.layout(tab), before);
                shell.open_app_search(window, cx);
                shell.open_review(
                    crate::git_tools::review::ReviewTab::File {
                        cwd: project.clone(),
                        path: file.to_string_lossy().into_owned(),
                    },
                    window,
                    cx,
                );
                let review = shell.workspace.active.unwrap();
                assert!(shell.placement_allowed(DragSource::Pane(review), caller, Edge::Bottom));
                let snapshot = shell
                    .app_workspace_action(
                        caller,
                        &project,
                        "workspace.list",
                        &json!({}),
                        window,
                        cx,
                    )
                    .unwrap();
                assert!(snapshot["tabs"].as_array().unwrap().iter().any(|tab| {
                    tab["panes"].as_array().unwrap().iter().any(|pane| {
                        pane["paneId"] == json!(review)
                            && pane["kind"] == "file"
                            && pane["title"] == "report.txt"
                            && pane.get("sessionId").is_none()
                    })
                }));
            })
            .unwrap();
    }

    #[gpui::test]
    fn operator_workspace_commands_keep_project_scope_and_pane_identity(cx: &mut TestAppContext) {
        let (window, other, caller) = super::super::tests::window(cx);
        window
            .update(cx, |shell, window, cx| {
                shell.sessions[&caller]
                    .update(cx, |session, cx| session.enable_mode("operator", cx));
                shell.open(caller, window, cx);
                let project = shell.workspace.project.clone();
                assert!(
                    shell
                        .app_workspace_action(
                            caller,
                            "/work/another-project",
                            "workspace.list",
                            &json!({}),
                            window,
                            cx
                        )
                        .is_err()
                );
                assert!(
                    shell
                        .app_workspace_action(
                            other,
                            &project,
                            "workspace.list",
                            &json!({}),
                            window,
                            cx
                        )
                        .is_err()
                );
                let listed = shell
                    .app_workspace_action(
                        caller,
                        &project,
                        "workspace.list",
                        &json!({}),
                        window,
                        cx,
                    )
                    .unwrap();
                assert_eq!(listed["activePaneId"], caller);

                let opened = shell
                    .app_workspace_action(
                        caller,
                        &project,
                        "workspace.open",
                        &json!({"kind":"search","placement":"right","targetPaneId":caller}),
                        window,
                        cx,
                    )
                    .unwrap();
                let search = opened["paneId"].as_i64().unwrap();
                assert!(search < 0);
                assert_eq!(
                    shell.workspace.tab_of(search),
                    shell.workspace.tab_of(caller)
                );
                let tab = shell.workspace.tab_of(search).unwrap();
                assert_eq!(shell.workspace.layout(tab).leaf_ids().len(), 2);

                let listed = shell
                    .app_workspace_action(
                        caller,
                        &project,
                        "workspace.list",
                        &json!({}),
                        window,
                        cx,
                    )
                    .unwrap();
                assert!(listed["tabs"].as_array().unwrap().iter().any(|entry| {
                    entry["tabId"] == tab
                        && entry["panes"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|pane| pane["paneId"] == search && pane["kind"] == "search")
                }));
                assert!(
                    shell
                        .app_workspace_action(
                            caller,
                            &project,
                            "workspace.resize",
                            &json!({"tabId":tab,"sashPath":[9],"index":0,"ratio":0.5}),
                            window,
                            cx
                        )
                        .is_err()
                );
                assert!(
                    shell
                        .app_workspace_action(
                            caller,
                            &project,
                            "workspace.resize",
                            &json!({"tabId":tab,"sashPath":["bad"],"index":0,"ratio":0.5}),
                            window,
                            cx
                        )
                        .is_err()
                );

                shell
                    .app_workspace_action(
                        caller,
                        &project,
                        "workspace.focus",
                        &json!({"paneId":caller}),
                        window,
                        cx,
                    )
                    .unwrap();
                assert_eq!(shell.workspace.active, Some(caller));
                shell
                    .app_workspace_action(
                        caller,
                        &project,
                        "workspace.focus",
                        &json!({"paneId":search}),
                        window,
                        cx,
                    )
                    .unwrap();
                assert_eq!(shell.workspace.active, Some(search));
                shell
                    .app_workspace_action(
                        caller,
                        &project,
                        "workspace.close",
                        &json!({"paneId":search}),
                        window,
                        cx,
                    )
                    .unwrap();
                assert!(shell.workspace.tab_of(search).is_none());
                assert!(shell.workspace.tab_of(caller).is_some());

                shell.sessions[&caller].update(cx, |session, _| {
                    session.settings.access = vorkspace_core::agent::AccessMode::ReadOnly
                });
                assert!(
                    shell
                        .app_workspace_action(
                            caller,
                            &project,
                            "workspace.list",
                            &json!({}),
                            window,
                            cx
                        )
                        .is_ok()
                );
                assert!(
                    shell
                        .app_workspace_action(
                            caller,
                            &project,
                            "workspace.open",
                            &json!({"kind":"search"}),
                            window,
                            cx
                        )
                        .is_err()
                );
            })
            .unwrap();
    }
}
