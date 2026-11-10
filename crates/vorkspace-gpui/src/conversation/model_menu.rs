//! Model and effort choices, and the composer's menus that offer them.
use super::*;

fn next_settings_revision(current: u64) -> u64 {
    let wall = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    wall.max(current.saturating_add(1))
}

impl Conversation {
    pub(super) fn refresh_catalog(&mut self, cx: &mut Context<Self>) {
        if cfg!(test) {
            return;
        }
        let kind = Kind::from_id(&self.summary.provider);
        self.attach_account(cx);
        self.catalog_epoch += 1;
        let epoch = self.catalog_epoch;
        let account = self.settings.account.clone();
        let work = cx.background_executor().spawn({
            let remote_project = self.is_remote().then(|| self.summary.cwd.clone());
            async move {
                if let Some(project) = remote_project {
                    vorkspace_core::catalog::remote_models(&project, kind.id())
                } else {
                    catalog::models_for(kind, &account)
                }
            }
        });
        cx.spawn(async move |this, cx| {
            if let Ok(models) = work.await {
                let _ = this.update(cx, |this, cx| {
                    if Kind::from_id(&this.summary.provider) == kind && this.catalog_epoch == epoch
                    {
                        this.catalog = models;
                        if this.is_remote() && this.model.is_none() {
                            this.model = this
                                .catalog
                                .iter()
                                .find(|m| m.default)
                                .or_else(|| this.catalog.first())
                                .map(|m| m.id.clone());
                            this.settings.model = this.model.clone();
                            this.save_settings(cx);
                        }
                        cx.emit(SessionChanged::Configuration);
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }
    pub(super) fn model_info(&self) -> Option<&Model> {
        match self.model.as_deref().or(self.resolved_model.as_deref()) {
            Some(id) => self.catalog.iter().find(|m| m.id == id),
            None => self
                .catalog
                .iter()
                .find(|m| m.default)
                .or_else(|| self.catalog.first()),
        }
    }
    /// A new tab shows the model and access it was created with from its
    /// first frame instead of flipping when the stored settings load.
    pub(crate) fn seed_new_settings(&mut self, settings: Settings, cx: &mut Context<Self>) {
        if self.loaded || self.settings.revision != 0 {
            return;
        }
        if settings.model.is_some() {
            self.model = settings.model.clone();
        }
        self.settings = settings;
        cx.notify();
    }
    pub(super) fn save_settings(&mut self, cx: &mut Context<Self>) {
        if self.history_partial {
            self.defer_history_action(|this, cx| this.save_settings(cx), cx);
            return;
        }
        self.settings.revision = next_settings_revision(self.settings.revision);
        let mut settings = self.settings.clone();
        settings.model = self.model.clone();
        let store = self.store.clone();
        let id = self.summary.id;
        let provider = self.summary.provider.clone();
        let remote = self.is_remote();
        let cancelled = self.settings_write_cancel.clone();
        let title = (!self.summary.has_messages).then(|| self.summary.title.clone());
        cx.background_executor()
            .spawn(async move {
                let saved = locked(&store).and_then(|s| {
                    if cancelled.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    s.configure_agent(id, &provider, &settings)?;
                    if let Some(title) = title
                        && s.agent_preferences(id)?
                            .is_some_and(|saved| saved.revision == settings.revision)
                    {
                        s.title(id, &title)?;
                    }
                    Ok(())
                });
                if saved.is_ok() && remote && !cancelled.load(Ordering::Acquire) {
                    let _ = vorkspace_runtime::runtime::remote::configure_saved(store, id);
                }
            })
            .detach();
    }
    pub(super) fn menu_choice(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.reject_user_entry(cx) {
            return;
        }

        if let Some(choice) = id.strip_prefix("recent-model:") {
            if let Ok((provider, model)) = serde_json::from_str::<(String, Option<String>)>(choice)
                && let Some(kind) = Kind::parse(&provider)
            {
                let picked = ModelPicked(kind, model);
                if self.apply_model_pick(&picked, cx) {
                    remember_model(kind, picked.1, cx);
                }
            }
            return;
        }
        if let Some(id) = id.strip_prefix("usage-account:") {
            if let Some(center) = crate::accounts::get(cx) {
                let kind = Kind::from_id(&self.summary.provider);
                if center.read(cx).book.exists(kind, id) {
                    let profile = center.read(cx).profile(id);
                    self.switch_usage_account(profile, cx);
                }
            }
            return;
        }
        if std::env::var_os("VORKSPACE_NATIVE_TRACE").is_some() {
            eprintln!("native settings session={} action={id}", self.summary.id);
        }
        if let Some(mode) = id.strip_prefix("access:") {
            self.settings.access = AccessMode::from_id(mode);
            remember_access(self.settings.access, cx);
        } else if let Some(effort) = id.strip_prefix("effort:") {
            self.set_effort_choice(effort, cx);
        } else if let Some(tier) = id.strip_prefix("tier:") {
            self.settings.service_tier = (!tier.is_empty()).then(|| tier.to_owned());
            self.settings.model_settings.remove("serviceTier");
        } else if let Some(setting) = id.strip_prefix("model-setting:") {
            if let Ok((key, value)) = serde_json::from_str::<(String, String)>(setting) {
                if is_effort_key(&key)
                    && (!self.is_remote() || matches!(key.as_str(), "effort" | "reasoningEffort"))
                {
                    self.set_effort_choice(&value, cx);
                }
                self.settings.model_settings.insert(key, value);
            }
        } else if let Some(key) = id.strip_prefix("model-toggle:") {
            let current = self
                .settings
                .model_settings
                .get(key)
                .map(String::as_str)
                .or_else(|| {
                    self.model_info()
                        .and_then(|model| model.controls.iter().find(|c| c.id == key))
                        .map(|c| c.current.as_str())
                });
            let value = if current == Some("true") {
                "false"
            } else {
                "true"
            };
            self.settings
                .model_settings
                .insert(key.to_owned(), value.to_owned());
        } else {
            match id {
                "workspace:current" => {
                    self.settings.worktree_removed = false;
                    self.settings.new_worktree = false;
                    self.settings.worktree_cwd = None;
                    self.refresh_git(cx);
                }
                "workspace:new" => {
                    self.settings.worktree_removed = false;
                    self.settings.new_worktree = true;
                    self.settings.worktree_cwd = None;
                    self.settings.worktree_base =
                        self.settings.worktree_base.clone().or(self.branch.clone());
                    self.refresh_git(cx);
                }
                "fast" => self.settings.fast = !self.settings.fast,
                "fast:on" => self.settings.fast = true,
                "fast:off" => self.settings.fast = false,
                "thinking" => {
                    self.settings.thinking = Some(!self.settings.thinking.unwrap_or(false))
                }
                "plan" | "operator" | "orchestrator" | "draft" => {
                    self.toggle_mode(id, cx);
                    return;
                }
                "queue" => {
                    self.bot_ui.queue = !self.bot_ui.queue;
                    cx.notify();
                    return;
                }
                _ => return,
            }
        }
        self.save_settings(cx);
        cx.notify();
    }
    pub(super) fn set_effort_choice(&mut self, effort: &str, cx: &mut Context<Self>) {
        self.settings
            .model_settings
            .retain(|key, _| !is_effort_key(key));
        self.settings.effort = (!effort.is_empty()).then(|| effort.to_owned());
        let provider = self.summary.provider.clone();
        crate::preference_writes::remember(cx, |prefs| {
            if effort.is_empty() {
                prefs.provider_efforts.remove(&provider);
            } else {
                prefs.provider_efforts.insert(provider, effort.to_owned());
            }
        });
    }
    pub(super) fn selected_effort(&self) -> Option<&str> {
        self.settings
            .effort
            .as_deref()
            .or_else(|| {
                [
                    "effort",
                    "reasoning",
                    "reasoningEffort",
                    "reasoning_effort",
                    "variant",
                ]
                .into_iter()
                .find_map(|key| self.settings.model_settings.get(key).map(String::as_str))
            })
            .or_else(|| self.model_info().and_then(|m| m.default_effort.as_deref()))
            .or_else(|| {
                self.model_info()
                    .and_then(|m| m.efforts.first().map(String::as_str))
            })
    }
    pub(super) fn effort_label(&self) -> SharedString {
        effort_name(self.selected_effort().unwrap_or("default")).into()
    }
    pub(super) fn effort_setting_label(&self) -> &'static str {
        if Kind::from_id(&self.summary.provider) == Kind::Codex {
            "Reasoning"
        } else {
            "Effort"
        }
    }
    pub(super) fn selected_tier(&self) -> Option<&str> {
        self.settings
            .service_tier
            .as_deref()
            .or_else(|| {
                self.settings
                    .model_settings
                    .get("serviceTier")
                    .map(String::as_str)
            })
            .filter(|value| !value.is_empty() && *value != "default")
    }
    /// Combined ModelPicker trigger; beside mode renders these as separate pills.
    pub(super) fn trigger_effort_label(&self) -> Option<SharedString> {
        if let Some(model) = self.model_info() {
            if let Some(control) = model.controls.iter().find(|control| {
                !model_control_toggle(control)
                    && matches!(
                        control.id.as_str(),
                        "effort"
                            | "reasoning"
                            | "reasoningEffort"
                            | "reasoning_effort"
                            | "thinking"
                            | "variant"
                    )
            }) {
                let selected = self
                    .settings
                    .model_settings
                    .get(&control.id)
                    .unwrap_or(&control.current);
                return Some(
                    control
                        .choices
                        .iter()
                        .find(|(value, _)| value == selected)
                        .map(|(_, label)| label)
                        .unwrap_or(selected)
                        .clone()
                        .into(),
                );
            }
            return (!model.efforts.is_empty()).then(|| self.effort_label());
        }
        // Keep the saved effort visible while the live catalog is loading.
        self.settings.effort.as_ref().map(|_| self.effort_label())
    }
    pub(super) fn effort_items(&self) -> Vec<MenuAction> {
        self.model_info().map_or_else(Vec::new, |model| {
            model
                .efforts
                .iter()
                .map(|effort| {
                    MenuAction::new(format!("effort:{effort}"), effort_name(effort))
                        .checked(self.selected_effort() == Some(effort.as_str()))
                })
                .collect()
        })
    }
    /// The pill's label, which drops the catalog names' "Claude " prefix.
    pub(super) fn model_label(&self) -> SharedString {
        let name = self
            .model_info()
            .map(|m| model_display_name(Kind::from_id(&self.summary.provider), &m.id, &m.name))
            .unwrap_or_else(|| {
                model_name(
                    Kind::from_id(&self.summary.provider),
                    self.model.as_deref().or(self.resolved_model.as_deref()),
                )
            });
        if Kind::from_id(&self.summary.provider) == Kind::Claude {
            name.strip_prefix("Claude ")
                .map_or(name.clone(), |short| short.to_owned().into())
        } else {
            name
        }
    }
    /// A different provider is armed for the next submission. The running
    /// provider retains its controls, account and immutable turn identity.
    pub(crate) fn apply_model_pick(
        &mut self,
        picked: &ModelPicked,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.editing_last_turn.pending() || self.status == "Preparing handoff…" {
            return false;
        }
        let provider = picked.0.id();
        let changes_provider = self.summary.provider != provider;
        let changed_model = changes_provider || self.model != picked.1;
        let local_switch = changes_provider && !self.is_remote();
        if local_switch && self.needs_history_restore() {
            let picked = ModelPicked(picked.0, picked.1.clone());
            self.defer_history_action(
                move |this, cx| {
                    if this.apply_model_pick(&picked, cx) {
                        remember_model(picked.0, picked.1, cx);
                    }
                },
                cx,
            );
            return false;
        }

        let mut next = self.settings.clone();
        next.model = picked.1.clone();
        next.effort = preferred_effort(provider, cx);
        next.fast = false;
        next.thinking = None;
        next.service_tier = None;
        next.model_settings.clear();
        if changes_provider {
            next.account = crate::accounts::get(cx)
                .map(|center| center.read(cx).selected(picked.0, &self.summary.cwd))
                .unwrap_or_default();
            let id = self.summary.id;
            let saved = if local_switch {
                let transcript = self.transcript.read(cx).source_rows();
                if next.pending_provider_switch.is_none()
                    && (self.responding()
                        || self.summary.provider_id.is_some()
                        || transcript.iter().any(|row| row.is_prompt()))
                {
                    next.pending_provider_switch = Some(vorkspace_core::agent::ProviderSwitch {
                        from: self.summary.provider.clone(),
                        cwd: self.execution_cwd().to_owned(),
                        model: self.model.clone(),
                        model_settings: self.settings.model_settings.clone(),
                        effort: self.settings.effort.clone(),
                        fast: self.settings.fast,
                        service_tier: self.settings.service_tier.clone(),
                        thinking: self.settings.thinking,
                        skip_outgoing_recap: self.settings.usage_limit.is_some(),
                        account: self.settings.account.clone(),
                        provider_id: self.summary.provider_id.clone(),
                    });
                }
                next.handoff = None;
                next.revision = next_settings_revision(next.revision);
                locked(&self.store).and_then(|store| {
                    store.select_local_provider(id, &self.summary.provider, picked.0, next)
                })
            } else {
                locked(&self.store).and_then(|store| {
                    store.change_provider(id, provider)?;
                    Ok((next, None))
                })
            };
            let (settings, binding) = match saved {
                Ok(saved) => saved,
                Err(error) => {
                    self.status = error;
                    cx.notify();
                    return false;
                }
            };
            next = settings;
            self.summary.provider_id = binding;
            self.summary.provider = provider.into();
            self.completion
                .update(cx, |completion, cx| completion.set_provider(provider, cx));
            self.catalog.clear();
            if !self.summary.has_messages {
                self.summary.title = format!("New {} session", picked.0.label());
            }
        }
        self.settings = next;
        self.model = picked.1.clone();
        self.resolved_model = None;
        if let Some(context) = &mut self.context {
            context.window = None;
            let context = *context;
            let store = self.store.clone();
            let id = self.summary.id;
            let account = self.settings.account.id.clone();
            cx.background_executor()
                .spawn(async move {
                    let _ = locked(&store).and_then(|s| {
                        s.save_context_usage_if_owner(id, &context, provider, &account)
                    });
                })
                .detach();
        }
        if changed_model && self.settings.usage_limit.is_some() && !self.responding() {
            self.recover_usage_after_model_change(cx);
        }
        self.refresh_catalog(cx);
        // The local provider and its handoff are already durable. Blanks
        // also save their descriptive title through the ordinary settings path.
        if !local_switch || !self.summary.has_messages {
            self.save_settings(cx);
        }
        cx.emit(SessionChanged::Configuration);
        true
    }
    pub(super) fn menu_is(&self, kind: ComposerMenu) -> bool {
        self.menu.as_ref().is_some_and(|open| open.kind == kind)
    }
    pub(super) fn anchor(&self, kind: ComposerMenu) -> &AnchorSlot {
        match kind {
            ComposerMenu::Add => &self.add_anchor,
            ComposerMenu::Model | ComposerMenu::Recent => &self.model_anchor,
            ComposerMenu::Access => &self.access_anchor,
            ComposerMenu::Account => &self.account_anchor,
            ComposerMenu::Workspace => &self.workspace_anchor,
            ComposerMenu::CompactSettings => &self.compact_anchor,
            ComposerMenu::Setting(ix) => self
                .bot_ui
                .setting_anchors
                .get(ix)
                .unwrap_or(&self.model_anchor),
        }
    }
    pub(super) fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        window.focus(&self.input.focus_handle(cx), cx);
        cx.notify();
    }
    pub(super) fn toggle_menu(
        &mut self,
        kind: ComposerMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.reject_user_entry(cx) {
            return;
        }

        if kind == ComposerMenu::Model
            && cx
                .try_global::<vorkspace_ui::preferences::Preferences>()
                .is_some_and(|p| p.model_controls == "beside")
        {
            self.toggle_bot_models(window, cx);
            return;
        }
        self.bot_ui.models = None;
        if self.menu.as_ref().is_some_and(|open| open.kind == kind) {
            self.close_menu(window, cx);
            return;
        }
        // Another menu gives way at once rather than overlapping this one.
        self.menu = None;
        self.completion
            .update(cx, |completion, cx| completion.dismiss(cx));
        self.git_picker = None;
        self.git_events = None;
        let mut events = Vec::new();
        let (label, items, width, style) = self.menu_items(kind, &mut events, window, cx);
        // The recent models open on the current one.
        let current = (kind == ComposerMenu::Recent)
            .then(|| {
                items.iter().find_map(|item| match item {
                    MenuItem::Action(action) if action.checked => Some(action.id.clone()),
                    _ => None,
                })
            })
            .flatten();
        let menu = cx.new(|cx| {
            let menu = Menu::new(label, items, window, cx)
                .width(width)
                .style(style);
            match current {
                Some(id) => menu.select_value(&id),
                None => menu,
            }
        });
        if kind == ComposerMenu::Model {
            menu.update(cx, |menu, cx| menu.open_lone_flyout(window, cx));
        }
        events.push(cx.subscribe_in(
            &menu,
            window,
            move |this, menu, event: &MenuEvent, window, cx| {
                if let MenuEvent::Picked(id, _) = event {
                    if id.as_ref() == "compact:branch" {
                        let new = !this.summary.has_messages && this.settings.new_worktree;
                        this.open_git(
                            if new { GitMode::Base } else { GitMode::Branch },
                            window,
                            cx,
                        );
                        return;
                    }
                    if id.as_ref() == "compact:cancel-edit" {
                        this.menu = None;
                        this.cancel_edit_last_turn(cx);
                        window.focus(&this.input.focus_handle(cx), cx);
                        cx.notify();
                        return;
                    }
                    let toggle = menu.read(cx).toggle_state(id);
                    if id.as_ref() == "upload" {
                        this.choose_attachments(cx);
                    } else {
                        this.menu_choice(id, cx);
                    }
                    if let Some(on) = toggle {
                        menu.update(cx, |menu, cx| menu.set_toggle(id, !on, cx));
                        cx.notify();
                        return;
                    }
                }
                this.menu = None;
                if kind == ComposerMenu::CompactSettings {
                    window.focus(&this.input.focus_handle(cx), cx);
                }
                cx.notify();
            },
        ));
        self.menu = Some(OpenMenu {
            kind,
            menu,
            _events: events,
        });
        cx.notify();
    }
    pub(super) fn menu_items(
        &self,
        kind: ComposerMenu,
        events: &mut Vec<Subscription>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (&'static str, Vec<MenuItem>, f32, MenuStyle) {
        let provider = Kind::from_id(&self.summary.provider);
        match kind {
            ComposerMenu::CompactSettings => {
                let bot = self.settings.bot.is_some();
                let draft = !self.summary.has_messages;
                let enabled = !self.responding() && !self.preparing_worktree && self.git.is_some();
                let mut items = vec![MenuItem::Header(
                    if bot {
                        "Bot settings"
                    } else {
                        "Session settings"
                    }
                    .into(),
                )];
                if !bot {
                    let linked = self.git.as_ref().is_some_and(|s| s.linked)
                        || self.settings.worktree_cwd.is_some();
                    let label = if draft && self.settings.new_worktree {
                        "New worktree"
                    } else if linked {
                        "Worktree"
                    } else {
                        "Current checkout"
                    };
                    items.push(MenuItem::Header(format!("Workspace · {label}").into()));
                    if draft {
                        let (_, workspace, _, _) =
                            self.menu_items(ComposerMenu::Workspace, events, window, cx);
                        items.extend(workspace.into_iter().filter_map(|item| match item {
                            MenuItem::Action(mut action) => {
                                action.disabled |= !enabled;
                                Some(MenuItem::Action(action))
                            }
                            _ => None,
                        }));
                    }
                    if let Some(branch) = if draft && self.settings.new_worktree {
                        self.settings
                            .worktree_base
                            .as_ref()
                            .or(self.branch.as_ref())
                    } else {
                        self.branch.as_ref()
                    } {
                        items.push(
                            MenuAction::new(
                                "compact:branch",
                                if draft && self.settings.new_worktree {
                                    "Worktree base"
                                } else {
                                    "Branch"
                                },
                            )
                            .value(
                                if !(draft && self.settings.new_worktree)
                                    && self.git.as_ref().is_some_and(|git| git.detached)
                                {
                                    format!("detached {branch}")
                                } else if draft && self.settings.new_worktree {
                                    crate::git_tools::git_picker::base_label(branch).to_owned()
                                } else {
                                    branch.clone()
                                },
                            )
                            .disabled(!enabled)
                            .into(),
                        );
                    }
                }
                let (_, model, _, _) = self.menu_items(ComposerMenu::Model, events, window, cx);
                // Combined mode already contains model options; beside mode must expose them here too.
                if cx
                    .try_global::<vorkspace_ui::preferences::Preferences>()
                    .is_some_and(|p| p.model_controls == "beside")
                {
                    items.extend(self.bot_model_options());
                }
                items.extend(model);
                if !bot {
                    let actions = self
                        .bot_access_options()
                        .into_iter()
                        .filter_map(|item| match item {
                            MenuItem::Action(action) => Some(*action),
                            _ => None,
                        })
                        .collect();
                    items.push(
                        MenuAction::new("compact:access", "Access")
                            .value(self.settings.access.label())
                            .submenu(actions)
                            .into(),
                    );
                }
                if self.editing_last_turn.pending() {
                    items.push(MenuAction::new("compact:cancel-edit", "Cancel edit").into());
                }
                if !bot
                    && self.status != "Ready"
                    && !self.responding()
                    && !self.loading
                    && self.settings.usage_limit.is_none()
                {
                    items.push(MenuItem::Note(self.status.clone().into()));
                }
                ("Session settings", items, 280., MenuStyle::Settings)
            }
            ComposerMenu::Account => {
                let items = crate::accounts::get(cx)
                    .map(|center| {
                        center
                            .read(cx)
                            .book
                            .list(provider)
                            .into_iter()
                            .map(|account| {
                                MenuAction::new(
                                    format!("usage-account:{}", account.id),
                                    account.label,
                                )
                                .checked(account.id == self.settings.account.id)
                                .into()
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                ("Switch account", items, 250., MenuStyle::Explorer)
            }
            ComposerMenu::Workspace => {
                let picker = cx.new(|cx| {
                    GitPicker::new(
                        self.summary.cwd.clone().into(),
                        GitMode::Worktree,
                        Some(self.execution_cwd().to_owned()),
                        cx,
                    )
                    .existing_worktrees()
                });
                events.push(cx.subscribe_in(
                    &picker,
                    window,
                    |this, _, picked: &GitPicked, window, cx| match picked {
                        GitPicked::Worktree(path) => {
                            if this.use_existing_worktree(path, cx) {
                                this.close_menu(window, cx);
                            }
                        }
                        GitPicked::Dismissed => this.close_menu(window, cx),
                        _ => {}
                    },
                ));
                let focus = picker.read(cx).focus(cx);
                (
                    "Workspace",
                    vec![
                        MenuItem::Header("Workspace".into()),
                        MenuAction::new("workspace:current", "Current checkout")
                            .icon("folder")
                            .checked(
                                !self.settings.new_worktree && self.settings.worktree_cwd.is_none(),
                            )
                            .into(),
                        MenuAction::new("workspace:new", "New worktree")
                            .icon("folder-tree")
                            .checked(self.settings.new_worktree)
                            .into(),
                        MenuItem::Separator,
                        MenuAction::new("workspace:existing", "Existing worktrees")
                            .icon("folder-tree")
                            .flyout(Flyout {
                                view: picker.into(),
                                width: 300.,
                                padding: 0.,
                                height: None,
                                max_height: Some(320.),
                                focus: Some(focus),
                                surface: None,
                            })
                            .into(),
                    ],
                    240.,
                    MenuStyle::Settings,
                )
            }
            // Composer.tsx's plus menu with native mode and attachment actions.
            ComposerMenu::Add => {
                let mut items = vec![MenuItem::Header("Add to message".into())];
                items.extend(
                    [
                        MenuAction::new("upload", "Upload file")
                            .icon("file-add")
                            .description("Attach files or images"),
                        MenuAction::new("plan", "Plan mode")
                            .icon("ai-idea")
                            .icon_color(tint(YELLOW_300, 0.8))
                            .description("Review a plan before building"),
                        MenuAction::new("operator", "Operator")
                            .disabled(self.is_remote())
                            .icon("cursor-magic")
                            .icon_color(tint(SKY_300, 0.8))
                            .description("Give this thread access to vorkspace"),
                        MenuAction::new("orchestrator", "Orchestrator")
                            .disabled(self.is_remote())
                            .icon("share")
                            .icon_color(tint(FUCHSIA_300, 0.65))
                            .badge("v1")
                            .description("Plan and coordinate agent work"),
                        MenuAction::new("draft", "Draft")
                            .icon("circle-dashed-hi")
                            .description("Save it without starting the agent")
                            .disabled(
                                self.follow_ups.state.draft.is_some()
                                    || (self.is_remote() && self.turn_in_flight()),
                            ),
                    ]
                    .into_iter()
                    .filter(|action| {
                        self.settings.bot.is_none()
                            || !matches!(action.id.as_ref(), "plan" | "operator" | "orchestrator")
                    })
                    .map(|action| {
                        let checked = match action.id.as_ref() {
                            "plan" => self.settings.plan,
                            "draft" => self.draft_active(cx),
                            "operator" => self.settings.operator,
                            "orchestrator" => self.settings.orchestrator,
                            _ => false,
                        };
                        action.checked(checked).into()
                    }),
                );
                if self.settings.bot.is_some() {
                    items.push(
                        MenuAction::new("queue", "Queue")
                            .icon("queue-list")
                            .description("Send after the current reply instead of steering it")
                            .checked(self.bot_ui.queue)
                            .into(),
                    );
                }
                ("Add to message", items, 250., MenuStyle::Composer)
            }
            // ModelPicker: a "Model" row whose flyout lists the models.
            ComposerMenu::Model => {
                let list = cx.new(|cx| {
                    let hidden = cx
                        .try_global::<vorkspace_ui::preferences::Preferences>()
                        .map(|p| p.hidden_providers.clone())
                        .unwrap_or_default();
                    ModelList::new_for(
                        provider,
                        self.model.clone(),
                        &self.summary.cwd,
                        self.settings.account.clone(),
                        cx,
                    )
                    .hide_providers(hidden)
                    .allow_provider_switch(true)
                });
                events.push(cx.subscribe_in(
                    &list,
                    window,
                    |this, _, picked: &ModelPicked, window, cx| {
                        if this.apply_model_pick(picked, cx) {
                            this.close_menu(window, cx);
                        }
                    },
                ));
                let focus = list.read(cx).search_focus(cx);
                let mut items = if cx
                    .try_global::<vorkspace_ui::preferences::Preferences>()
                    .is_none_or(|p| p.model_controls != "beside")
                {
                    self.bot_model_options()
                } else {
                    Vec::new()
                };
                items.push(
                    MenuAction::new("model", "Model")
                        .value_image(provider.id())
                        .value(self.model_info().map_or_else(
                            || model_name(provider, self.model.as_deref()),
                            |model| model_display_name(provider, &model.id, &model.name),
                        ))
                        .flyout(Flyout {
                            view: list.into(),
                            width: 310.,
                            padding: 0.,
                            height: Some(442.),
                            max_height: None,
                            focus: Some(focus),
                            surface: None,
                        })
                        .into(),
                );
                ("Model and settings", items, 250., MenuStyle::Settings)
            }
            ComposerMenu::Recent => (
                "Recently used models",
                self.recent_model_items(cx),
                250.,
                MenuStyle::Recent,
            ),
            ComposerMenu::Access => {
                let items = self.bot_access_options();
                ("Access", items, 288., MenuStyle::Access)
            }
            ComposerMenu::Setting(ix) => self.bot_setting_menu(ix),
        }
    }
    /// The composer's "+" menu, for captures.
    #[cfg(test)]
    pub(crate) fn toggle_add_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_menu(ComposerMenu::Add, window, cx);
    }
    /// The model trigger: the composer's model menu.
    #[cfg(test)]
    pub(crate) fn toggle_model_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.reject_user_entry(cx) {
            return;
        }

        if self.settings.bot.is_some() {
            self.toggle_bot_models(window, cx);
            return;
        }
        self.toggle_menu(ComposerMenu::Model, window, cx);
    }
    /// `App: Switch Model` (Cmd+.): toggles the recent models, as ModelPicker's
    /// hotkey does.
    pub(crate) fn toggle_recent_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.reject_user_entry(cx) {
            return;
        }

        self.bot_ui.models = None;
        self.toggle_menu(ComposerMenu::Recent, window, cx);
    }
    /// Right-clicking the model trigger opens the recent models over any menu.
    pub(super) fn open_recent_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.menu_is(ComposerMenu::Recent) {
            self.toggle_recent_models(window, cx);
        }
    }
    /// recentMenuModels: up to six remembered choices, with the current model
    /// added when it is not among them.
    pub(super) fn recent_model_items(&self, cx: &gpui::App) -> Vec<MenuItem> {
        let provider = Kind::from_id(&self.summary.provider);
        let current = (provider, self.model.clone());
        let hidden = cx
            .try_global::<vorkspace_ui::preferences::Preferences>()
            .map(|p| p.hidden_providers.clone())
            .unwrap_or_default();
        let mut choices: Vec<(Kind, Option<String>)> = cx
            .try_global::<vorkspace_ui::preferences::Preferences>()
            .into_iter()
            .flat_map(|p| &p.recent_models)
            .filter_map(|choice| {
                let kind = Kind::parse(&choice.provider)?;
                choice.model.as_ref()?;
                let selected = (kind, choice.model.clone()) == current;
                let model_hidden = cx
                    .try_global::<vorkspace_ui::preferences::Preferences>()
                    .is_some_and(|p| p.model_hidden(kind.id(), choice.model.as_deref().unwrap()));
                ((kind == provider || !hidden.iter().any(|id| id == kind.id()))
                    && (selected || !model_hidden))
                    .then(|| (kind, choice.model.clone()))
            })
            .collect();
        choices.dedup();
        if !choices.iter().take(6).any(|choice| choice == &current) {
            choices.truncate(5);
            choices.push(current.clone());
        }
        choices.truncate(6);
        choices
            .into_iter()
            .map(|(kind, model)| {
                let selected = (kind, model.clone()) == current;
                let name = if selected {
                    self.model_label()
                } else {
                    let name = cached_model_name(kind, model.as_deref());
                    match name.strip_prefix("Claude ") {
                        Some(short) if kind == Kind::Claude => short.to_owned().into(),
                        _ => name,
                    }
                };
                let id = serde_json::to_string(&(kind.id(), &model)).unwrap_or_default();
                MenuAction::new(format!("recent-model:{id}"), name)
                    .image(kind.id())
                    .description(kind.label())
                    .checked(selected)
                    .into()
            })
            .collect()
    }
}
