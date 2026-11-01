use super::*;
use crate::popover::MenuOpened;
use gpui::App;
use std::collections::{HashMap, HashSet};
use vorkspace_core::{
    accounts::Profile,
    response::{Target, TransferKind},
};

type Catalogs = Vec<(Kind, Option<Result<Vec<Model>, String>>)>;

pub(super) struct ResponseMenu {
    pub(super) menu: Entity<Menu>,
    pub(super) anchor: AnchorSlot,
    pub(super) _events: Vec<Subscription>,
    pub(super) regions: Vec<AnchorSlot>,
    models: HashMap<Kind, Entity<Menu>>,
    profiles: HashMap<Kind, Profile>,
    requested: HashSet<Kind>,
}

impl Conversation {
    pub(super) fn open_response_menu(
        &mut self,
        turn: usize,
        kind: TransferKind,
        token: Instant,
        anchor: AnchorSlot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.make_response_menu(turn, kind, anchor.clone(), None, window, cx);
        let remote = self.is_remote().then(|| self.summary.cwd.clone());
        let hidden = cx
            .try_global::<vorkspace_ui::preferences::Preferences>()
            .map(|prefs| prefs.hidden_providers.clone())
            .unwrap_or_default();
        let work = cx.background_executor().spawn(async move {
            if let Some(project) = remote {
                return catalog::remote_catalog(&project).map(|mut catalog| {
                    catalog
                        .providers
                        .into_iter()
                        .map(|provider| {
                            let rows = if let Some(error) = catalog.errors.remove(&provider) {
                                Err(error)
                            } else {
                                Ok(catalog.models.remove(&provider).unwrap_or_default())
                            };
                            (provider, Some(rows))
                        })
                        .collect()
                });
            }
            // Discover executables only. Starting every CLI here made one slow
            // provider delay the entire picker, including cached catalogs.
            Ok(Kind::ALL
                .into_iter()
                .filter(|provider| !hidden.iter().any(|id| id == provider.id()))
                .filter(|provider| vorkspace_core::provider::resolve(*provider).is_some())
                .map(|provider| (provider, None))
                .collect::<Catalogs>())
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.response_request != Some((turn, kind, token)) {
                    return;
                }
                match result {
                    Ok(catalogs) => {
                        this.make_response_menu(turn, kind, anchor, Some(catalogs), window, cx)
                    }
                    Err(error) => {
                        if let Some(menu) = &this.response_menu {
                            menu.menu.update(cx, |menu, cx| {
                                menu.set_items(vec![MenuItem::Note(error.into())], cx)
                            });
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn make_response_menu(
        &mut self,
        turn: usize,
        kind: TransferKind,
        anchor: AnchorSlot,
        catalogs: Option<Catalogs>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let from = self.response_provider(turn, cx);
        let label = if kind == TransferKind::Handoff {
            "Hand this session to another agent"
        } else {
            "Send this turn to another agent"
        };
        let mut items = vec![MenuItem::Note(
            if kind == TransferKind::Handoff {
                "Hand this session to another agent to continue the work."
            } else {
                "Send this turn to another agent to review the work."
            }
            .into(),
        )];
        if catalogs.is_none() {
            items.push(MenuItem::Note("Loading providers…".into()));
        }
        let mut events = Vec::new();
        let mut regions = Vec::new();
        let mut menus = HashMap::new();
        let mut profiles = HashMap::new();
        let mut requested = HashSet::new();
        for (provider, initial) in catalogs.into_iter().flatten() {
            if kind == TransferKind::Handoff && provider.id() == from {
                continue;
            }
            let profile = if provider.id() == self.summary.provider {
                self.settings.account.clone()
            } else {
                crate::accounts::get(cx)
                    .map(|center| center.read(cx).selected(provider, &self.summary.cwd))
                    .unwrap_or_default()
            };
            if initial.is_some() {
                requested.insert(provider);
            }
            let cached = initial.or_else(|| {
                (!self.is_remote())
                    .then(|| catalog::cached_models_for(provider, &profile))
                    .flatten()
                    .map(Ok)
            });
            let options = cached
                .map(|result| self.response_model_items(turn, kind, provider, result, cx))
                .unwrap_or_else(|| vec![MenuItem::Note("Loading models…".into())]);
            let models = cx.new(|cx| {
                Menu::embedded(provider.label(), options, cx)
                    .width(240.)
                    .style(MenuStyle::Settings)
            });
            regions.push(models.read(cx).submenu_region());
            let surface = models.read(cx).surface_region();
            let focus = models.focus_handle(cx);
            events.push(cx.subscribe_in(
                &models,
                window,
                move |this, _, event: &MenuEvent, window, cx| match event {
                    MenuEvent::Picked(id, _) => this.finish_response_target(turn, kind, id, cx),
                    MenuEvent::Dismissed => {
                        this.close_response_menu(cx);
                        window.focus(&this.input.focus_handle(cx), cx);
                    }
                },
            ));
            menus.insert(provider, models.clone());
            profiles.insert(provider, profile);
            items.push(
                MenuAction::new(provider.id(), provider.label())
                    .image(provider.id())
                    .flyout(Flyout {
                        view: models.into(),
                        width: 240.,
                        padding: 4.,
                        height: None,
                        max_height: Some(288.),
                        focus: Some(focus),
                        surface: Some(surface),
                    })
                    .into(),
            );
        }
        if items.len() == 1 {
            items.push(MenuItem::Note("No other providers are available.".into()));
        }
        let menu = cx.new(|cx| {
            Menu::new(label, items, window, cx)
                .width(240.)
                .style(MenuStyle::Settings)
        });
        events.push(cx.subscribe_in(
            &menu,
            window,
            move |this, _, event: &MenuEvent, window, cx| match event {
                MenuEvent::Picked(id, _) => this.finish_response_target(turn, kind, id, cx),
                MenuEvent::Dismissed => {
                    this.close_response_menu(cx);
                    window.focus(&this.input.focus_handle(cx), cx);
                }
            },
        ));
        events.push(cx.subscribe(&menu, move |this, _, event: &MenuOpened, cx| {
            if let Some(provider) = Kind::parse(&event.0) {
                this.load_response_models(provider, cx);
            }
        }));
        self.response_menu = Some(ResponseMenu {
            menu,
            anchor,
            _events: events,
            regions,
            models: menus,
            profiles,
            requested,
        });
    }

    fn load_response_models(&mut self, provider: Kind, cx: &mut Context<Self>) {
        let Some(menu) = &mut self.response_menu else {
            return;
        };
        if !menu.requested.insert(provider) {
            return;
        }
        let Some(models) = menu.models.get(&provider).cloned() else {
            return;
        };
        // UI tests drive completion explicitly, without starting real CLIs.
        if cfg!(test) {
            return;
        }
        let profile = menu.profiles.get(&provider).cloned().unwrap_or_default();
        let request = self.response_request;
        let work = cx
            .background_executor()
            .spawn(async move { catalog::models_for(provider, &profile) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.finish_response_models(request, provider, &models, result, cx);
            });
        })
        .detach();
    }

    fn finish_response_models(
        &mut self,
        request: Option<(usize, TransferKind, Instant)>,
        provider: Kind,
        menu: &Entity<Menu>,
        result: Result<Vec<Model>, String>,
        cx: &mut Context<Self>,
    ) {
        // A closed/reopened picker must not receive an older request.
        let Some((turn, kind, _)) =
            request.filter(|request| self.response_request == Some(*request))
        else {
            return;
        };
        let items = self.response_model_items(turn, kind, provider, result, cx);
        menu.update(cx, |menu, cx| menu.set_items(items, cx));
        cx.notify();
    }

    fn response_provider(&self, turn: usize, cx: &App) -> String {
        self.transcript
            .read(cx)
            .turns
            .get(&turn)
            .map(|t| t.provider.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.summary.provider)
            .to_owned()
    }

    fn response_model_items(
        &self,
        turn: usize,
        kind: TransferKind,
        provider: Kind,
        result: Result<Vec<Model>, String>,
        cx: &App,
    ) -> Vec<MenuItem> {
        let models = match result {
            Ok(models) => models,
            Err(error) => {
                return vec![MenuItem::Note(
                    format!("Couldn't load models: {error}").into(),
                )];
            }
        };
        let from = self.response_provider(turn, cx);
        let transcript = self.transcript.read(cx);
        let source = transcript.turns.get(&turn);
        let mut options = Vec::new();
        for model in models {
            if cx
                .try_global::<vorkspace_ui::preferences::Preferences>()
                .is_some_and(|prefs| prefs.model_hidden(provider.id(), &model.id))
            {
                continue;
            }
            if kind == TransferKind::SecondOpinion
                && provider.id() == from
                && source.is_some_and(|source| {
                    let source_id = source.model_id.as_deref().unwrap_or(&source.model);
                    let source_id = source_id
                        .strip_prefix(&format!("{from}:"))
                        .unwrap_or(source_id);
                    [
                        Some(model.id.as_str()),
                        model.alias_target.as_deref(),
                        Some(model.name.as_str()),
                        Some(model.name.strip_prefix("Claude ").unwrap_or(&model.name)),
                    ]
                    .into_iter()
                    .flatten()
                    .any(|candidate| {
                        model_ids_match(provider, source_id, candidate)
                            || source.model.eq_ignore_ascii_case(candidate)
                    })
                })
            {
                continue;
            }
            let target = |effort: Option<String>| {
                serde_json::to_string(&Target {
                    provider: provider.id().into(),
                    model: Some(model.id.clone()),
                    effort,
                    settings: model
                        .controls
                        .iter()
                        .map(|control| (control.id.clone(), control.current.clone()))
                        .collect(),
                })
                .unwrap()
            };
            let mut option = MenuAction::new(
                target(None),
                model_display_name(provider, &model.id, &model.name),
            );
            if !model.efforts.is_empty() {
                let mut efforts = vec![MenuAction::new(target(None), "Default")];
                efforts.extend(model.efforts.iter().map(|effort| {
                    MenuAction::new(target(Some(effort.clone())), effort_name(effort))
                }));
                option = option.submenu(efforts);
            }
            options.push(option.into());
        }
        if options.is_empty() {
            options.push(MenuItem::Note("No different models are available.".into()));
        }
        options
    }
}

fn model_ids_match(provider: Kind, left: &str, right: &str) -> bool {
    let normalize = |id: &str| {
        let id = id
            .strip_prefix(&format!("{}:", provider.id()))
            .unwrap_or(id);
        let id = id.strip_suffix("[1m]").unwrap_or(id);
        if provider == Kind::Claude {
            id.strip_prefix("claude-")
                .unwrap_or(id)
                .replace('.', "-")
                .to_lowercase()
        } else {
            id.to_lowercase()
        }
    };
    normalize(left) == normalize(right)
}

#[cfg(test)]
mod tests;
