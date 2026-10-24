//! Unsaved Bot setup. Only confirmation admits an identity to the roster.
use super::*;
use vorkspace_core::{
    bot::InitialConfiguration,
    catalog::{self, Model},
};

pub(super) struct Creation {
    pub bot: Bot,
    pub provider: Kind,
    pub settings: Settings,
    pub soul: String,
    pub catalog: Vec<Model>,
    pub generation: u64,
    pub window: gpui::AnyWindowHandle,
    pub previous_focus: Option<gpui::FocusHandle>,
    pub surface: Entity<CreationView>,
}
pub(crate) struct CreationView {
    owner: WeakEntity<Bots>,
    _observe: Subscription,
}
impl Render for CreationView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.owner
            .upgrade()
            .and_then(|owner| {
                owner.update(cx, |owner, cx| owner.render_creation_surface(window, cx))
            })
            .unwrap_or_else(|| div().into_any_element())
    }
}
impl Creation {
    pub fn model(&self) -> Option<&Model> {
        match self.settings.model.as_deref() {
            Some(id) => self.catalog.iter().find(|model| model.id == id),
            None => self
                .catalog
                .iter()
                .find(|model| model.default)
                .or(self.catalog.first()),
        }
    }
}
impl Bots {
    #[cfg(test)]
    pub(crate) fn create_named(
        &mut self,
        projects: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.create(projects, window, cx);
        self.creation.as_mut().unwrap().bot.name =
            format!("Test bot {}", self.roster.bots.len() + 1);
        self.confirm_creation(window, cx);
        self.settings = Some(SettingsSection::General);
    }
    pub(crate) fn creating(&self) -> bool {
        self.creation.is_some()
    }
    pub(crate) fn focus_creation(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self
            .creation
            .as_ref()
            .is_some_and(|draft| draft.window.window_id() == window.window_handle().window_id())
        {
            return false;
        }
        if self.picker.is_none()
            && self.menu.is_none()
            && matches!(self.settings, None | Some(SettingsSection::General))
            && let Some(name) = self.fields.get("bot-name")
        {
            window.focus(&name.focus_handle(cx), cx);
        }
        true
    }
    pub(super) fn settings_identity(&self) -> Option<String> {
        self.creation
            .as_ref()
            .map(|draft| draft.bot.id.clone())
            .or(self.active.clone())
    }

    pub(crate) fn create(
        &mut self,
        projects: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(draft) = &self.creation {
            let handle = draft.window;
            let id = draft.bot.id.clone();
            let requesting_window = window.window_handle();
            let owner = cx.entity().downgrade();
            let field = self.fields.get("bot-name").cloned();
            cx.defer(move |cx| {
                if !owner
                    .update(cx, |owner, _| {
                        owner.creation.as_ref().is_some_and(|draft| {
                            draft.bot.id == id && draft.window.window_id() == handle.window_id()
                        })
                    })
                    .unwrap_or(false)
                {
                    return;
                }
                let focused = handle.update(cx, |_, window, cx| {
                    window.activate_window();
                    if let Some(field) = field {
                        window.focus(&field.focus_handle(cx), cx);
                    }
                });
                if focused.is_err() {
                    let _ = requesting_window.update(cx, |_, window, cx| {
                        let _ = owner.update(cx, |owner, cx| {
                            // A cancelled/replaced draft must never be reclaimed by
                            // a delayed window activation from its predecessor.
                            if !owner.creation.as_ref().is_some_and(|draft| {
                                draft.bot.id == id && draft.window.window_id() == handle.window_id()
                            }) {
                                return;
                            }
                            owner.save_open_file(cx);
                            let draft = owner.creation.as_mut().unwrap();
                            draft.window = requesting_window;
                            draft.previous_focus = window.focused(cx);
                            window.activate_window();
                            owner.focus_creation(window, cx);
                            cx.notify();
                        });
                    });
                }
            });
            return;
        }
        self.save_open_file(cx);
        self.settings = None;
        self.picker = None;
        self.menu = None;
        self.fields.remove("bot-name");
        let provider = self
            .prefs
            .default_provider
            .as_deref()
            .and_then(Kind::parse)
            .unwrap_or(Kind::Codex);
        let settings = Settings {
            access: AccessMode::Auto,
            model: self.prefs.default_models.get(provider.id()).cloned(),
            effort: crate::conversation::preferred_effort(provider.id(), cx),
            ..Default::default()
        };
        let owner = cx.entity();
        let surface = cx.new(|cx| CreationView {
            owner: owner.downgrade(),
            _observe: cx.observe(&owner, |_, _, cx| cx.notify()),
        });
        self.creation = Some(Creation {
            bot: self.roster.draft(projects),
            provider,
            settings,
            soul: bot::DEFAULT_SOUL.into(),
            catalog: Vec::new(),
            generation: 0,
            window: window.window_handle(),
            previous_focus: window.focused(cx),
            surface,
        });
        let field = self.field("bot-name", "Bot name", String::new(), false, cx);
        field.update(cx, |field, cx| {
            field.set_max_chars(40);
            field.set_placeholder("Name your bot", cx);
        });
        self.observations.push(cx.subscribe_in(
            &field,
            window,
            |this, input, _: &input::Submitted, window, cx| {
                if this
                    .fields
                    .get("bot-name")
                    .is_some_and(|current| current.entity_id() == input.entity_id())
                {
                    this.confirm_creation(window, cx);
                }
            },
        ));
        window.focus(&field.focus_handle(cx), cx);
        self.load_creation_catalog(cx);
        cx.notify();
    }

    pub(crate) fn confirm_creation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.creation.as_ref() else {
            return;
        };
        if draft.bot.name.trim().is_empty()
            || draft.window.window_id() != window.window_handle().window_id()
        {
            return;
        }
        self.save_open_file(cx);
        let Some(draft) = self.creation.as_ref() else {
            return;
        };
        let mut bot = draft.bot.clone();
        bot.initial_configuration = Some(Box::new(InitialConfiguration {
            provider: draft.provider.id().into(),
            settings: draft.settings.clone(),
            soul: Some(draft.soul.clone()),
        }));
        let id = match self.roster.create(bot) {
            Ok(bot) => bot.id.clone(),
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        self.creation = None;
        self.settings = None;
        self.fields.remove("bot-name");
        let soul = format!("{id}:SOUL.md");
        self.fields.remove(&format!("file:{soul}"));
        self.file_versions.remove(&soul);
        self.awaiting_creation.insert(id.clone());
        // Persist the named identity and its configuration before any backing
        // session becomes authoritative; ensure_chat publishes its ID last.
        self.save(cx);
        self.open(&id, window, cx);
        self.changed(cx);
    }

    pub(super) fn cancel_creation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(draft) = self.creation.take() {
            self.settings = None;
            self.picker = None;
            self.menu = None;
            self.fields.remove("bot-name");
            self.fields.retain(|key, _| !key.contains(&draft.bot.id));
            self.file_versions
                .retain(|key, _| !key.starts_with(&draft.bot.id));
            if let Some(focus) = draft.previous_focus {
                window.focus(&focus, cx);
            }
            cx.notify();
        }
    }

    pub(super) fn settings_bot(&self, id: &str) -> Option<&Bot> {
        self.creation
            .as_ref()
            .filter(|draft| draft.bot.id == id)
            .map(|draft| &draft.bot)
            .or_else(|| self.roster.get(id))
    }
    pub(super) fn edit_settings_bot(
        &mut self,
        id: &str,
        edit: impl FnOnce(&mut Bot),
        cx: &mut Context<Self>,
    ) {
        if let Some(draft) = self.creation.as_mut().filter(|draft| draft.bot.id == id) {
            edit(&mut draft.bot);
            draft.bot.normalize();
            cx.notify();
        } else if let Some(bot) = self.roster.get_mut(id) {
            edit(bot);
            bot.normalize();
            self.configure_chat(id, cx);
            self.save(cx);
        }
    }
    pub(super) fn load_creation_catalog(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.creation.as_mut() else {
            return;
        };
        draft.generation += 1;
        let (id, provider, account, generation) = (
            draft.bot.id.clone(),
            draft.provider,
            draft.settings.account.clone(),
            draft.generation,
        );
        if let Some(cached) = catalog::cached_models_for(provider, &account) {
            draft.catalog = cached;
        }
        if cfg!(test) {
            return;
        }
        let work = cx
            .background_executor()
            .spawn(async move { catalog::models_for(provider, &account) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                let Some(draft) = this.creation.as_mut().filter(|draft| {
                    draft.bot.id == id
                        && draft.generation == generation
                        && draft.provider == provider
                }) else {
                    return;
                };
                if let Ok(catalog) = result {
                    draft.catalog = catalog;
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn choose_creation_option(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(draft) = &mut self.creation {
            let model = draft.model().cloned();
            crate::conversation::bot_settings::apply_model_option(
                &mut draft.settings,
                model.as_ref(),
                id,
            );
            cx.notify();
        }
    }

    pub(crate) fn creation_surface(&self, window: &Window) -> Option<Entity<CreationView>> {
        self.creation
            .as_ref()
            .filter(|draft| draft.window.window_id() == window.window_handle().window_id())
            .map(|draft| draft.surface.clone())
    }

    fn render_creation_surface(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let draft = self
            .creation
            .as_ref()
            .filter(|draft| draft.window.window_id() == window.window_handle().window_id())?;
        let bot = draft.bot.clone();
        Some(
            div()
                .absolute()
                .size_full()
                .on_action(
                    cx.listener(|this, _: &crate::shell::DismissMenu, window, cx| {
                        if this.picker.take().is_none() && this.menu.take().is_none() {
                            this.cancel_creation(window, cx);
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape"
                        && this.picker.is_none()
                        && this.menu.is_none()
                    {
                        this.cancel_creation(window, cx);
                        cx.stop_propagation();
                    }
                }))
                .children(self.settings_modal(&bot, window, cx))
                .children(self.picker_surface(cx))
                .into_any_element(),
        )
    }
}
