//! Stateless Bot configuration controls, shared by setup drafts and live chats.
use super::*;

pub(crate) fn selected_effort<'a>(
    model: Option<&'a Model>,
    settings: &'a Settings,
) -> Option<&'a str> {
    settings
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
            .find_map(|key| settings.model_settings.get(key).map(String::as_str))
        })
        .or_else(|| model.and_then(|model| model.default_effort.as_deref()))
        .or_else(|| model.and_then(|model| model.efforts.first().map(String::as_str)))
}
fn selected_tier(settings: &Settings) -> Option<&str> {
    settings
        .service_tier
        .as_deref()
        .or_else(|| {
            settings
                .model_settings
                .get("serviceTier")
                .map(String::as_str)
        })
        .filter(|value| !value.is_empty() && *value != "default")
}
pub(crate) fn model_options(
    provider: Kind,
    model: Option<&Model>,
    settings: &Settings,
) -> Vec<MenuItem> {
    let mut items: Vec<MenuAction> = Vec::new();
    if model.is_some_and(|m| !m.efforts.is_empty()) {
        items.push(
            MenuAction::new(
                "reasoning",
                if provider == Kind::Codex {
                    "Reasoning"
                } else {
                    "Effort"
                },
            )
            .value(effort_name(
                selected_effort(model, settings).unwrap_or("default"),
            ))
            .submenu(
                model
                    .into_iter()
                    .flat_map(|model| &model.efforts)
                    .map(|effort| {
                        MenuAction::new(format!("effort:{effort}"), effort_name(effort))
                            .checked(selected_effort(model, settings) == Some(effort.as_str()))
                    })
                    .collect(),
            ),
        );
    }
    if model.is_some_and(|m| m.fast) {
        items.push(MenuAction::new("fast", "Fast").toggle(settings.fast));
    }
    if let Some(model) = model.filter(|m| !m.tiers.is_empty()) {
        let mut tiers =
            vec![MenuAction::new("tier:", "Standard").checked(selected_tier(settings).is_none())];
        tiers.extend(model.tiers.iter().map(|(id, name)| {
            MenuAction::new(format!("tier:{id}"), name.clone())
                .checked(selected_tier(settings) == Some(id.as_str()))
        }));
        items.push(
            MenuAction::new("service-tier", "Service Tier")
                .value(
                    selected_tier(settings)
                        .and_then(|id| model.tiers.iter().find(|(key, _)| key == id))
                        .map(|(_, label)| label.as_str())
                        .unwrap_or_else(|| selected_tier(settings).unwrap_or("Standard"))
                        .to_owned(),
                )
                .submenu(tiers),
        );
    }
    if model.is_some_and(|m| m.thinking) {
        items.push(
            MenuAction::new("thinking", "Thinking").toggle(settings.thinking.unwrap_or(false)),
        );
    }
    if let Some(model) = model {
        for control in &model.controls {
            let selected = settings
                .model_settings
                .get(&control.id)
                .unwrap_or(&control.current);
            let label = match control.id.as_str() {
                "effort" | "reasoning" => "Effort",
                _ => &control.name,
            };
            if model_control_toggle(control) {
                items.push(
                    MenuAction::new(format!("model-toggle:{}", control.id), label.to_owned())
                        .toggle(selected == "true"),
                );
                continue;
            }
            let choices = control
                .choices
                .iter()
                .map(|(value, name)| {
                    let key = serde_json::to_string(&(control.id.clone(), value.clone())).unwrap();
                    MenuAction::new(format!("model-setting:{key}"), name.clone())
                        .checked(selected == value)
                })
                .collect();
            items.push(
                MenuAction::new(format!("model-control:{}", control.id), label.to_owned())
                    .value(
                        control
                            .choices
                            .iter()
                            .find(|(value, _)| value == selected)
                            .map(|(_, name)| name)
                            .unwrap_or(selected)
                            .clone(),
                    )
                    .submenu(choices),
            );
        }
    }
    items.sort_by_key(|action| {
        let key = action
            .id
            .strip_prefix("model-control:")
            .or_else(|| action.id.strip_prefix("model-toggle:"))
            .unwrap_or(&action.id);
        match key {
            "fast" => 0,
            "lead" => 1,
            "effort" => 2,
            "sidekick" => 3,
            "reasoning" if provider == Kind::Codex => 5,
            "reasoning" => 4,
            "reasoningEffort" | "reasoning_effort" => 5,
            "service-tier" | "serviceTier" => 6,
            "thinking" => 7,
            "variant" => 8,
            "agent" => 9,
            "context" => 10,
            _ => 99,
        }
    });
    items.into_iter().map(MenuItem::from).collect()
}
pub(crate) fn access_options(provider: Kind, access: AccessMode, busy: bool) -> Vec<MenuItem> {
    let mut items = vec![
        MenuAction::new("access:read-only", "Read-only")
            .icon("square-lock")
            .description("Inspect without changing files.")
            .checked(access == AccessMode::ReadOnly)
            .into(),
    ];
    items.extend(ACCESS_MODES.iter().map(|(id, label, hint, glyph)| {
        let hint = match (provider, *id) {
            (Kind::Pi | Kind::Omp, _) => {
                "Tool confirmations follow this provider's configured extensions."
            }
            _ => *hint,
        };
        let action = MenuAction::new(format!("access:{id}"), *label)
            .icon(glyph)
            .description(hint)
            .checked(access.id() == *id);
        MenuItem::from(if *id == "full-access" {
            action.icon_color(status(StatusColor::Warning, 1.))
        } else {
            action
        })
    }));
    if busy {
        items.push(MenuItem::Note("Changes apply to the next turn.".into()));
    }
    items
}
pub(crate) fn apply_model_option(settings: &mut Settings, model: Option<&Model>, id: &str) {
    if let Some(effort) = id.strip_prefix("effort:") {
        settings.model_settings.retain(|key, _| !is_effort_key(key));
        settings.effort = (!effort.is_empty()).then(|| effort.to_owned());
    } else if let Some(tier) = id.strip_prefix("tier:") {
        settings.service_tier = (!tier.is_empty()).then(|| tier.to_owned());
        settings.model_settings.remove("serviceTier");
    } else if let Some(setting) = id.strip_prefix("model-setting:") {
        if let Ok((key, value)) = serde_json::from_str::<(String, String)>(setting) {
            if is_effort_key(&key) {
                settings.model_settings.retain(|key, _| !is_effort_key(key));
                settings.effort = (!value.is_empty()).then(|| value.clone());
            }
            settings.model_settings.insert(key, value);
        }
    } else if let Some(key) = id.strip_prefix("model-toggle:") {
        let current = settings
            .model_settings
            .get(key)
            .map(String::as_str)
            .or_else(|| {
                model
                    .and_then(|model| model.controls.iter().find(|control| control.id == key))
                    .map(|control| control.current.as_str())
            });
        let value = if current == Some("true") {
            "false"
        } else {
            "true"
        };
        settings
            .model_settings
            .insert(key.to_owned(), value.to_owned());
    } else if id == "fast" {
        settings.fast = !settings.fast;
    } else if id == "thinking" {
        settings.thinking = Some(!settings.thinking.unwrap_or(false));
    }
}
