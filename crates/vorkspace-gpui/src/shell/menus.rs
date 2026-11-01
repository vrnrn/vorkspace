//! The shell's popover menus: what they offer and what each item does.
use super::*;

impl Shell {
    pub(super) fn menu_open(&self, target: &MenuTarget) -> bool {
        self.menu
            .as_ref()
            .is_some_and(|open| open.target == *target)
    }
    pub(super) fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            self.clear_context_selection();
            self.invalidate_chrome(cx);
            cx.notify();
        }
    }
    pub(crate) fn open_menu(
        &mut self,
        target: MenuTarget,
        anchor: Anchor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(&target, MenuTarget::Project { .. }) {
            self.scan_external_editors(cx);
        }
        self.close_preview(cx);
        // Close the previous menu first, so the new one remembers the focus
        // that was there before either opened.
        self.menu = None;
        let mut events = Vec::new();
        let (label, items, width, style) = self.menu_items(&target, &mut events, window, cx);
        let menu = cx.new(|cx| {
            Menu::new(label, items, window, cx)
                .width(width)
                .style(style)
        });
        events.push(
            cx.subscribe_in(&menu, window, |this, _, event: &MenuEvent, window, cx| {
                if let MenuEvent::Picked(id, _) = event
                    && let Some(open) = this
                        .menu
                        .as_ref()
                        .filter(|m| m.target == MenuTarget::SidebarOptions)
                {
                    let menu = open.menu.clone();
                    this.change_sidebar_option(id, cx);
                    let items = this.sidebar_options_menu_items(cx);
                    menu.update(cx, |menu, cx| menu.replace_items(items, cx));
                    return;
                }
                if let MenuEvent::Picked(id, _) = event
                    && id != "archived"
                    && let Some(open) = this
                        .menu
                        .as_ref()
                        .filter(|m| m.target == MenuTarget::SessionFilter)
                {
                    let menu = open.menu.clone();
                    this.change_session_filter(id, cx);
                    let items = this.filter_menu_items(cx);
                    menu.update(cx, |menu, cx| menu.replace_items(items, cx));
                    return;
                }
                let Some(open) = this.menu.take() else {
                    return;
                };
                let target = open.target.clone();
                if let MenuEvent::Picked(id, row) = event {
                    this.run_menu(target, id, *row, window, cx);
                }
                this.clear_context_selection();
                this.invalidate_chrome(cx);
                cx.notify();
            }),
        );
        if target == MenuTarget::SidebarOptions {
            events.push(cx.subscribe_in(
                &menu,
                window,
                |this, sender, event: &crate::popover::MenuReordered, _, cx| {
                    if !this.menu.as_ref().is_some_and(|open| {
                        open.target == MenuTarget::SidebarOptions && &open.menu == sender
                    }) {
                        return;
                    }
                    this.reorder_sidebar_navigation(event, cx);
                    let items = this.sidebar_options_menu_items(cx);
                    sender.update(cx, |menu, cx| menu.replace_items(items, cx));
                },
            ));
        }
        self.menu = Some(ShellMenu {
            target,
            anchor,
            menu,
            _events: events,
        });
        self.invalidate_chrome(cx);
        cx.notify();
    }
    pub(super) fn menu_items(
        &self,
        target: &MenuTarget,
        events: &mut Vec<Subscription>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (SharedString, Vec<MenuItem>, f32, MenuStyle) {
        if let Some(menu) = self.terminal_menu_items(target, cx) {
            return menu;
        }
        let title = |id: &i64| {
            self.session_summaries
                .get(id)
                .map(|_| self.session_metadata(*id, cx).display_title())
                .unwrap_or_default()
        };
        match target {
            // Answered by terminal_menu_items above.
            MenuTarget::TerminalTab(_) => ("".into(), Vec::new(), 160., MenuStyle::Explorer),
            MenuTarget::Tab(_) | MenuTarget::TerminalTitleTab(_) => {
                let (tab, label) = match target {
                    MenuTarget::Tab(id) => (TitleTab::Session(*id), title(id)),
                    MenuTarget::TerminalTitleTab(tab) => (tab.clone(), "Terminal".into()),
                    _ => unreachable!(),
                };
                let tabs = self.visible_tabs(cx);
                let ix = tabs.iter().position(|entry| *entry == tab).unwrap_or(0);
                let count = tabs.len();
                let closable = tab.session().is_none_or(|id| self.tab_can_close(id, cx));
                let mut items = vec![
                    MenuAction::new("close", "Close Tab")
                        .shortcut(format!("{MOD}W"))
                        .disabled(!closable)
                        .into(),
                    MenuItem::Separator,
                    MenuAction::new("others", "Close Other Tabs")
                        .disabled(count < 2)
                        .into(),
                    MenuAction::new("right", "Close Tabs to the Right")
                        .disabled(ix + 1 >= count)
                        .into(),
                    MenuAction::new("left", "Close Tabs to the Left")
                        .disabled(ix == 0)
                        .into(),
                    MenuAction::new("all", "Close All Tabs")
                        .shortcut("⌘⇧W")
                        .disabled(count < 2 && !closable)
                        .into(),
                ];
                // Closing a tab keeps its session; archive also hides it from
                // the active history while preserving everything on disk.
                if let Some(id) = tab.session().filter(|id| {
                    self.session_summaries.contains_key(id)
                        && self.session_metadata(*id, cx).summary.has_messages
                }) {
                    items.extend([
                        MenuItem::Separator,
                        MenuAction::new(
                            "archive",
                            if self.session_metadata(id, cx).summary.archived {
                                "Unarchive"
                            } else {
                                "Archive"
                            },
                        )
                        .into(),
                        MenuAction::new("delete", "Delete").danger().into(),
                    ]);
                }
                (
                    format!("Tab actions for {label}").into(),
                    items,
                    244.,
                    MenuStyle::Explorer,
                )
            }
            MenuTarget::Session(id) => (
                format!("Session actions for {}", title(id)).into(),
                self.session_menu_items(*id, cx),
                228.,
                MenuStyle::Explorer,
            ),
            MenuTarget::Folder(folder) => (
                "Folder actions".into(),
                self.folder_menu_items(folder, events, cx),
                228.,
                MenuStyle::Explorer,
            ),
            MenuTarget::TerminalDock => {
                let side = self
                    .workspace
                    .docks
                    .get(&self.workspace.project)
                    .map_or(DockSide::Bottom, |d| d.side);
                (
                    "Move terminal".into(),
                    [
                        ("bottom", "Dock Bottom", DockSide::Bottom),
                        ("top", "Dock Top", DockSide::Top),
                        ("left", "Dock Left", DockSide::Left),
                        ("right", "Dock Right", DockSide::Right),
                    ]
                    .into_iter()
                    .map(|(id, label, s)| MenuAction::new(id, label).checked(s == side).into())
                    .collect(),
                    160.,
                    MenuStyle::Explorer,
                )
            }
            MenuTarget::SessionFilter => (
                "Filter sessions".into(),
                self.filter_menu_items(cx),
                228.,
                MenuStyle::Filter,
            ),
            MenuTarget::SidebarOptions => (
                "Sidebar options".into(),
                self.sidebar_options_menu_items(cx),
                212.,
                MenuStyle::Filter,
            ),
            // SidebarWorktreeSwitcher's menu under the project header.
            MenuTarget::Header => {
                let picker = cx.new(|cx| {
                    ProjectPicker::new(&self.workspace.projects, &self.workspace.project, cx)
                        .appearance(self.workspace.project_appearance.clone())
                        .allow_remote()
                });
                events.push(cx.subscribe_in(
                    &picker,
                    window,
                    |this, _, picked: &ProjectPicked, window, cx| {
                        this.close_menu(cx);
                        match picked {
                            ProjectPicked::Project(path) if *path != this.workspace.project => {
                                this.select_project(path.clone(), window, cx)
                            }
                            ProjectPicked::Project(_) => {}
                            ProjectPicked::New => this.open_project(cx),
                            ProjectPicked::Remote => {
                                this.open_remote_project(window, cx);
                                return;
                            }
                        }
                        this.focus_active(window, cx);
                        this.invalidate_chrome(cx);
                        cx.notify();
                    },
                ));
                let focus = picker.read(cx).search_focus(cx);
                let branch = self
                    .worktree_focus
                    .get(&self.workspace.project)
                    .and_then(|focus| focus.branch.clone())
                    .or_else(|| {
                        crate::conversation::git_branch(std::path::Path::new(
                            &self.workspace.project,
                        ))
                    });
                let worktree = MenuAction::new("switch-worktree", "Switch worktree")
                    .icon("folder-tree")
                    .flyout(self.worktree_flyout(events, window, cx));
                (
                    format!("{} menu", self.project_label(&self.workspace.project)).into(),
                    vec![
                        match branch {
                            Some(branch) => worktree.value(branch),
                            None => worktree,
                        }
                        .into(),
                        MenuItem::Separator,
                        MenuAction::new("switch-project", "Switch project")
                            .icon("folder")
                            .flyout(Flyout {
                                view: picker.into(),
                                width: 286.,
                                padding: 0.,
                                height: None,
                                max_height: Some(380.),
                                focus: Some(focus),
                                surface: None,
                            })
                            .into(),
                        MenuAction::new("project-settings", "Project settings")
                            .icon("sliders")
                            .flyout(self.project_settings_flyout(events, window, cx))
                            .into(),
                        MenuAction::new("open-remote-project", "Open folder on a machine…")
                            .icon("globe")
                            .into(),
                        MenuAction::new("settings", "Settings")
                            .icon("settings")
                            .shortcut(format!("{MOD},"))
                            .into(),
                    ],
                    240.,
                    MenuStyle::Switcher,
                )
            }
            // TabGroupMenu's appearance controls and project actions.
            MenuTarget::Project { path, rail } => {
                let controls = self.project_appearance_controls(path, events, window, cx);
                let mut items: Vec<MenuItem> = vec![
                    MenuItem::Custom(controls.clone().into(), controls.read(cx).focus(cx)),
                    MenuItem::Separator,
                    MenuAction::new("background", "Background image")
                        .icon("image-add")
                        .into(),
                ];
                if *rail {
                    let rail = self
                        .workspace
                        .project_rail_state
                        .clone()
                        .unwrap_or_default();
                    let key = vorkspace_core::files::path_key(path);
                    let mut groups = vec![
                        MenuAction::new("project-group:new", "New group…"),
                        MenuAction::new("project-group:set:", "No group")
                            .checked(!rail.assignments.contains_key(&key)),
                    ];
                    groups.extend(rail.groups.iter().map(|g| {
                        MenuAction::new(format!("project-group:set:{}", g.id), g.name.clone())
                            .checked(rail.assignments.get(&key) == Some(&g.id))
                    }));
                    let manage = rail
                        .groups
                        .iter()
                        .flat_map(|g| {
                            [
                                MenuAction::new(
                                    format!("group-rename:{}", g.id),
                                    format!("Rename {}…", g.name),
                                ),
                                MenuAction::new(
                                    format!("group-remove:{}", g.id),
                                    format!("Remove {}", g.name),
                                ),
                            ]
                        })
                        .collect::<Vec<_>>();
                    items.push(
                        MenuAction::new("project-group", "Move to group")
                            .icon("folder-tree")
                            .submenu(groups)
                            .into(),
                    );
                    if !manage.is_empty() {
                        items.push(
                            MenuAction::new("manage-groups", "Manage groups")
                                .icon("folder-tree")
                                .submenu(manage)
                                .into(),
                        );
                    }
                    items.push(
                        MenuAction::new(
                            "pin",
                            if rail.pinned.contains(&key) {
                                "Unpin project"
                            } else {
                                "Pin project"
                            },
                        )
                        .icon("pin")
                        .into(),
                    );
                }
                items.extend([
                    MenuAction::new("reveal", REVEAL_LABEL)
                        .icon("folder-open")
                        .into(),
                    self.external_editor_menu(path).into(),
                    MenuItem::Separator,
                    self.project_notification_menu(path).into(),
                    MenuItem::Separator,
                    MenuAction::new("archive", "Archive").icon("archive").into(),
                    MenuAction::new("delete", "Delete")
                        .icon("trash")
                        .danger()
                        .into(),
                ]);
                (
                    format!("Project actions for {}", project_name(path)).into(),
                    items,
                    260.,
                    MenuStyle::Group,
                )
            }
        }
    }
    pub(super) fn run_menu(
        &mut self,
        target: MenuTarget,
        action: &str,
        row: Option<Bounds<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if action == "archive"
            && let MenuTarget::Tab(id) = target
        {
            self.toggle_archive(id, cx);
            return;
        }
        if action == "delete"
            && let MenuTarget::Tab(id) = target
        {
            self.request_delete_sessions(vec![id], window, cx);
            return;
        }
        match target {
            MenuTarget::TerminalTab(ix) => self.run_terminal_tab_menu(ix, action, window, cx),
            MenuTarget::TerminalDock => {
                let side = match action {
                    "top" => DockSide::Top,
                    "left" => DockSide::Left,
                    "right" => DockSide::Right,
                    _ => DockSide::Bottom,
                };
                self.set_dock_side(side, cx);
            }
            MenuTarget::SessionFilter => self.change_session_filter(action, cx),
            MenuTarget::SidebarOptions => self.change_sidebar_option(action, cx),
            MenuTarget::Tab(id) => {
                self.run_title_tab_menu(TitleTab::Session(id), action, window, cx)
            }
            MenuTarget::TerminalTitleTab(tab) => self.run_title_tab_menu(tab, action, window, cx),
            MenuTarget::Session(id) => self.run_session_menu(id, action, window, cx),
            MenuTarget::Folder(folder) => self.run_folder_menu(folder, action, window, cx),
            MenuTarget::Header => {
                // "Project settings" opens the project's menu beside its row.
                if action == "settings" {
                    self.open_settings(crate::settings::Section::General, window, cx);
                } else if action == "open-remote-project" {
                    self.open_remote_project(window, cx);
                } else if action == "project-settings" {
                    let at = row
                        .map_or_else(Point::default, |r| Point::new(r.right(), r.top() - px(8.)));
                    let target = MenuTarget::Project {
                        path: self.workspace.project.clone(),
                        rail: false,
                    };
                    self.open_menu(target, Anchor::Point(at), window, cx);
                }
            }
            MenuTarget::Project { path, .. } => {
                if (action == "pin"
                    || action.starts_with("project-group:")
                    || action.starts_with("group-rename:")
                    || action.starts_with("group-remove:"))
                    && let Err(error) = self.workspace.migrate_project_rail()
                {
                    self.error = Some(format!(
                        "Cannot change project groups: {error}. Existing preferences were kept."
                    ));
                    cx.notify();
                    return;
                }
                if action == "pin" {
                    let key = vorkspace_core::files::path_key(&path);
                    let rail = self.workspace.project_rail_mut();
                    let value = if rail.pinned.remove(&key) {
                        false
                    } else {
                        rail.pinned.insert(key.clone());
                        true
                    };
                    if rail.awaiting_legacy_pins {
                        rail.pin_overrides.insert(key, value);
                    }
                    self.persist(cx);
                    self.invalidate_chrome(cx);
                    cx.notify();
                } else if action == "project-group:new" {
                    let rail = self.workspace.project_rail_mut();
                    let id = format!("native-group-{}", vorkspace_core::bot::new_id());
                    rail.groups.push(crate::workspace::ProjectRailGroup {
                        id: id.clone(),
                        name: "New group".into(),
                        collapsed: false,
                    });
                    let key = vorkspace_core::files::path_key(&path);
                    rail.assignments.insert(key.clone(), id.clone());
                    if rail.awaiting_legacy_assignments {
                        rail.assignment_overrides
                            .insert(key.clone(), Some(id.clone()));
                    }
                    rail.pinned.remove(&key);
                    if rail.awaiting_legacy_pins {
                        rail.pin_overrides.insert(key, false);
                    }
                    self.persist(cx);
                    self.start_rename(session_groups::RenameTarget::ProjectGroup(id), window, cx);
                } else if let Some(id) = action.strip_prefix("group-rename:") {
                    self.start_rename(
                        session_groups::RenameTarget::ProjectGroup(id.into()),
                        window,
                        cx,
                    );
                } else if let Some(id) = action.strip_prefix("group-remove:") {
                    let rail = self.workspace.project_rail_mut();
                    rail.groups.retain(|g| g.id != id);
                    if rail.awaiting_legacy_assignments {
                        let removed: Vec<_> = rail
                            .assignments
                            .iter()
                            .filter(|(_, g)| g.as_str() == id)
                            .map(|(p, _)| p.clone())
                            .collect();
                        for path in removed {
                            rail.assignment_overrides.insert(path, None);
                        }
                    }
                    rail.assignments.retain(|_, g| g != id);
                    self.workspace.project_group_collapsed.remove(id);
                    self.persist(cx);
                    self.invalidate_chrome(cx);
                    cx.notify();
                } else if let Some(id) = action.strip_prefix("project-group:set:") {
                    let rail = self.workspace.project_rail_mut();
                    let key = vorkspace_core::files::path_key(&path);
                    if id.is_empty() {
                        rail.assignments.remove(&key);
                        if rail.awaiting_legacy_assignments {
                            rail.assignment_overrides.insert(key, None);
                        }
                    } else if rail.groups.iter().any(|g| g.id == id) {
                        rail.assignments.insert(key.clone(), id.into());
                        if rail.awaiting_legacy_assignments {
                            rail.assignment_overrides.insert(key, Some(id.into()));
                        }
                    }
                    self.persist(cx);
                    self.invalidate_chrome(cx);
                    cx.notify();
                } else if action == "archive" {
                    self.archive_project(&path, window, cx);
                } else if action == "delete" {
                    self.confirm_delete_project(path, window, cx);
                } else if action == "reveal" {
                    if path.starts_with("remote://") {
                        self.error = Some("Remote folders stay on their host machine.".into());
                        cx.notify();
                    } else {
                        cx.reveal_path(std::path::Path::new(&path));
                    }
                } else if let Some(editor) = action.strip_prefix("external-editor:") {
                    self.open_in_external_editor(editor, &path, cx);
                } else if action == "background" {
                    self.open_background_dialog(path, window, cx);
                } else if action.starts_with("mute:") {
                    self.run_project_notification_menu(&path, action, window, cx);
                }
            }
        }
    }
    // Archive only after the workspace has been durably saved. Existing tabs and
    // processes remain owned by their current lifecycle and are never closed here.
    pub(super) fn commit_project_archive(&mut self, next: Workspace, cx: &mut Context<Self>) {
        let state = serde_json::to_string(&next).expect("serialize workspace");
        // Cancel queued older snapshots before acquiring the store lock.
        self.generation.fetch_add(1, Ordering::SeqCst);
        let result = locked(&self.store).and_then(|store| store.save_ui_state(&state));
        if let Err(error) = result {
            self.error = Some(format!("Could not save project archive: {error}"));
        } else {
            *self.latest_state.lock().unwrap_or_else(|e| e.into_inner()) = state;
            self.workspace = next;
            self.invalidate_chrome(cx);
        }
        cx.notify();
    }
}
