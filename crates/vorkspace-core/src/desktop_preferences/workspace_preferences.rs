//! Desktop localStorage settings expressed in native workspace field names.
use super::{reconcile_preference, DesktopPreferences};
use serde_json::{json, Map, Value};

impl DesktopPreferences {
    /// Decode sidebar state retained by desktop versions that did not expose
    /// these fields natively. Existing canonical fields always take precedence;
    /// unavailable WebKit storage must not discard a saved native selection.
    pub fn restore_sidebar_preferences(workspace: &mut Value) {
        let Some(target) = workspace.as_object_mut() else {
            return;
        };
        let retained = retained_preferences(target);
        for (field, source) in retained.sidebar_preferences() {
            let value = target.get(field).cloned().unwrap_or(source);
            let value = match field {
                "project_rail_open" => json!(value.as_bool().unwrap_or(true)),
                "project_rail_width" => json!(value
                    .as_f64()
                    .filter(|width| width.is_finite())
                    .unwrap_or(200.)
                    .clamp(180., 360.)
                    .round()),
                "sidebar_tabs" => sidebar_tabs(&value, true),
                "sidebar_tab_order" => sidebar_tab_order(&value),
                _ => unreachable!(),
            };
            target.insert(field.into(), value);
        }
    }

    fn sidebar_preferences(&self) -> [(&'static str, Value); 4] {
        [
            (
                "project_rail_open",
                json!(!matches!(
                    self.get("vorkspace.projectRailOpen"),
                    Some("0" | "false")
                )),
            ),
            (
                "project_rail_width",
                json!(self
                    .number("projectRailWidth")
                    .unwrap_or(200.)
                    .clamp(180., 360.)
                    .round()),
            ),
            (
                "sidebar_tabs",
                sidebar_tabs(
                    &self
                        .json("vorkspace.projectSidebarTabs.v1")
                        .unwrap_or(Value::Null),
                    false,
                ),
            ),
            (
                "sidebar_tab_order",
                sidebar_tab_order(
                    &self
                        .json("vorkspace.sidebarTabOrder")
                        .unwrap_or(Value::Null),
                ),
            ),
        ]
    }

    pub(super) fn apply_preferences(
        &self,
        target: &mut Map<String, Value>,
        baseline: &Value,
        next: &mut Map<String, Value>,
    ) {
        for (field, key, default) in [
            ("sounds", "sounds", true),
            ("notifications", "notifications", false),
            ("tab_animations", "tabAnimationsEnabled", false),
            ("quick_composer", "quickComposerEnabled", false),
            ("working_agents", "liveAgentsEnabled", true),
            ("close_to_tray", "closeToTray", true),
            ("inset_glass", "insetGlass", true),
            ("body_glass", "bodyGlass", true),
            ("show_excluded_files", "showExcludedFiles", false),
            ("anchor_prompts", "transcriptAnchor", true),
            ("paced_reveal", "pacedReveal", true),
            ("autocorrect", "composerAutocorrect", true),
            ("format_on_save", "formatOnSave", true),
            ("autosave", "autosave", false),
            ("show_remaining_usage", "showRemainingUsage", false),
            ("mask_emails", "maskEmails", true),
            ("claude_hooks", "claudeHooks", true),
            ("bots", "botsEnabled", true),
            ("rail_bots_pinned", "railBotsPinned", false),
            ("bot_menu_bar_icon", "botMenuBarIcon", true),
            ("vox_enabled", "vox.enabled", false),
            ("vox_silent", "vox.silent", false),
        ] {
            reconcile_preference(
                target,
                baseline,
                next,
                field,
                json!(self.flag(key).unwrap_or(default)),
            );
        }
        let microphone = self
            .json("vorkspace.vox.microphone.v1")
            .filter(|value| {
                value["id"].as_str().is_some_and(|id| !id.is_empty()) && value["label"].is_string()
            })
            .unwrap_or_else(|| json!({"id":"builtin","label":"built-in microphone"}));
        reconcile_preference(target, baseline, next, "vox_microphone", microphone);
        for (field, key, allowed, default) in [
            (
                "theme",
                "colorScheme",
                &["dark", "light", "system"][..],
                "dark",
            ),
            (
                "file_tabs",
                "fileTabMode",
                &["pane", "workspace"][..],
                "pane",
            ),
            (
                "file_view_mode",
                "fileViewMode",
                &["preview", "source"][..],
                "preview",
            ),
            (
                "follow_up",
                "followUpBehavior",
                &["steer", "queue"][..],
                "steer",
            ),
            (
                "diff_view",
                "diffViewer",
                &["unified", "split", "editor"][..],
                "unified",
            ),
            (
                "transcript_layout",
                "transcriptLayout",
                &["chat", "full"][..],
                "chat",
            ),
            (
                "diff_palette",
                "diffPalette",
                &["default", "colorblind", "high-contrast"][..],
                "default",
            ),
            (
                "collapsed_project_rail",
                "collapsedProjectRailMode",
                &["compact", "hidden"][..],
                "compact",
            ),
            (
                "chat_background_scope",
                "chatBackgroundScope",
                &["empty", "all"][..],
                "all",
            ),
            (
                "chat_background_effect",
                "newThreadBackgroundEffect",
                &[
                    "none",
                    "dither",
                    "ascii",
                    "halftone",
                    "scanlines",
                    "gradient-blur",
                ][..],
                "none",
            ),
            (
                "workspace_theme",
                "workspaceTheme",
                &[
                    "default", "harbor", "moss", "ember", "orchid", "tidepool", "dune", "rosewood",
                    "glacier", "arcade", "ink",
                ][..],
                "default",
            ),
        ] {
            let value = self
                .get(&format!("vorkspace.{key}"))
                .filter(|value| allowed.contains(value))
                .unwrap_or(default);
            reconcile_preference(target, baseline, next, field, json!(value));
        }
        for (field, key, default, min, max, scale) in [
            ("theme_hue", "themeHue", 240., 0., 360., 1.),
            ("theme_saturation", "themeSaturation", 0., 0., 100., 1.),
            (
                "theme_dark_lightness",
                "themeDarkLightness",
                9.,
                0.,
                30.,
                1.,
            ),
            ("sidebar_opacity", "sidebarOpacity", 0.85, 0.15, 1., 100.),
            ("sidebar_blur", "sidebarBlur", 24., 1., 64., 1.),
            ("ui_scale", "uiScale", 1., 0.5, 2., 100.),
        ] {
            let mut number = self.number(key).unwrap_or(default).clamp(min, max);
            if field == "ui_scale" {
                number = (number * 10.).round() / 10.;
            }
            reconcile_preference(
                target,
                baseline,
                next,
                field,
                json!((number * scale).round() as u32),
            );
        }
        for (field, key) in [
            ("chat_background_empty", "chatBackgroundEmptyOpacity"),
            ("chat_background_session", "chatBackgroundSessionOpacity"),
        ] {
            let opacity = self
                .number(key)
                .or_else(|| self.number("chatBackgroundOpacity"))
                .unwrap_or(0.24)
                .clamp(0.05, 0.65);
            reconcile_preference(
                target,
                baseline,
                next,
                field,
                json!((opacity * 100.).round() as u32),
            );
        }
        reconcile_preference(
            target,
            baseline,
            next,
            "accent_color",
            json!(self.get("vorkspace.accentColor").filter(|v| valid_hex(v))),
        );
        reconcile_preference(
            target,
            baseline,
            next,
            "chat_background",
            json!(self
                .get("vorkspace.chatBackgroundPath")
                .filter(|v| !v.trim().is_empty())),
        );
        let idle = self
            .number("idleAgentLimit")
            .filter(|v| [0., 1., 2., 3., 5.].contains(v))
            .unwrap_or(3.);
        reconcile_preference(target, baseline, next, "idle_agents", json!(idle as u32));
        for (field, key, default) in [
            (
                "project_notifications",
                "vorkspace.projectNotifications.v1",
                json!({}),
            ),
            (
                "notification_projects",
                "vorkspace.notificationProjects.v2",
                json!([]),
            ),
        ] {
            let value = self
                .json(key)
                .filter(|value| {
                    if field == "project_notifications" {
                        value.is_object()
                    } else {
                        value.is_array()
                    }
                })
                .unwrap_or(default);
            reconcile_preference(target, baseline, next, field, value);
        }
        let paths: Map<String, Value> = self
            .json("vorkspace.providerBinaryPaths.v1")
            .as_ref()
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter(|(provider, _)| crate::kind::Kind::parse(provider).is_some())
            .filter_map(|(provider, value)| {
                value
                    .as_str()
                    .filter(|path| !path.trim().is_empty())
                    .map(|path| (provider.clone(), json!(path.trim())))
            })
            .collect();
        reconcile_preference(target, baseline, next, "binary_paths", json!(paths));
        let disabled = self
            .json("vorkspace.disabledSkillPaths")
            .as_ref()
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        reconcile_preference(
            target,
            baseline,
            next,
            "disabled_skill_paths",
            json!(disabled),
        );
        reconcile_preference(
            target,
            baseline,
            next,
            "default_models",
            json!(native_models(
                &self.json("vorkspace.defaultModels").unwrap_or(Value::Null)
            )),
        );
        reconcile_preference(
            target,
            baseline,
            next,
            "hidden_providers",
            json!(providers(
                &self
                    .json("vorkspace.hiddenPickerProviders")
                    .unwrap_or(Value::Null)
            )),
        );
        let filters = self.json("vorkspace.sessionSidebarFilters");
        let archived = filters
            .as_ref()
            .map(|v| v["showArchived"] == true)
            .unwrap_or_else(|| self.flag("sessionsShowArchived").unwrap_or(false));
        reconcile_preference(target, baseline, next, "show_archived", json!(archived));
        let keys: Map<String, Value> = self
            .json("vorkspace.keybindingOverrides")
            .as_ref()
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter_map(|(command, value)| {
                if let Some(shortcut) = value["shortcut"].as_str().and_then(shortcut) {
                    Some((command.clone(), json!(shortcut)))
                } else if value["disabled"] == true {
                    Some((command.clone(), Value::Null))
                } else {
                    None
                }
            })
            .collect();
        reconcile_preference(target, baseline, next, "keybindings", json!(keys));
        let favorites = self.json("vorkspace.favoriteModels");
        let mut choices = Vec::new();
        for id in favorites
            .as_ref()
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            let Some((provider, model)) = id.split_once(':') else {
                continue;
            };
            if let Some(choice) =
                model_choice(provider, model).filter(|choice| !choices.contains(choice))
            {
                choices.push(choice);
            }
        }
        reconcile_preference(target, baseline, next, "favorite_models", json!(choices));
        let recent = self.json("vorkspace.recentModels");
        let mut choices = Vec::new();
        for choice in recent
            .as_ref()
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let (Some(provider), Some(model)) =
                (choice["harness"].as_str(), choice["model"].as_str())
            else {
                continue;
            };
            if let Some(choice) =
                model_choice(provider, model).filter(|choice| !choices.contains(choice))
            {
                choices.push(choice);
            }
            if choices.len() == 6 {
                break;
            }
        }
        reconcile_preference(target, baseline, next, "recent_models", json!(choices));
        let tab = self
            .get("vorkspace.modelPickerTab")
            .filter(|tab| *tab == "favorites" || crate::kind::Kind::parse(tab).is_some())
            .unwrap_or("favorites");
        reconcile_preference(target, baseline, next, "model_picker_tab", json!(tab));
    }

    pub(super) fn apply_view_preferences(
        &self,
        target: &mut Map<String, Value>,
        baseline: &Value,
        next: &mut Map<String, Value>,
    ) {
        let old = baseline.get("workspace").unwrap_or(&Value::Null);
        let mut source = Map::new();
        let retained = retained_preferences(target);
        for ((field, value), (_, recovered)) in self
            .sidebar_preferences()
            .into_iter()
            .zip(retained.sidebar_preferences())
        {
            let source_key = match field {
                "project_rail_open" => "vorkspace.projectRailOpen",
                "project_rail_width" => "vorkspace.projectRailWidth",
                "sidebar_tabs" => "vorkspace.projectSidebarTabs.v1",
                "sidebar_tab_order" => "vorkspace.sidebarTabOrder",
                _ => unreachable!(),
            };
            if old.get(field).is_none()
                && !target.contains_key(field)
                && self.get(source_key).is_none()
                && retained.get(source_key).is_some()
            {
                target.insert(field.into(), recovered);
            }
            // These fields were absent from the first native schema. If a
            // canonical value already exists, establish the source baseline
            // without treating that explicit native value as stale desktop data.
            let initial = (old.get(field).is_none() && target.contains_key(field))
                .then(|| json!({(field):value.clone()}));
            reconcile_preference(
                target,
                initial.as_ref().unwrap_or(old),
                &mut source,
                field,
                value,
            );
        }
        reconcile_preference(
            target,
            old,
            &mut source,
            "new_layout_prompt_dismissed",
            json!(self.flag("newLayoutPromptDismissed").unwrap_or(false)),
        );
        // Only the original desktop layout is supported. Earlier saved inset
        // choices cannot reactivate the removed layout when reopening a store.
        target.insert("classic_layout".into(), json!(true));
        source.insert("classic_layout".into(), json!(true));
        reconcile_preference(
            target,
            old,
            &mut source,
            "sidebar_visible",
            json!(self.flag("sessionSidebarOpen").unwrap_or(true)),
        );
        reconcile_preference(
            target,
            old,
            &mut source,
            "right_panel_open",
            json!(self.flag("rightPanelOpen").unwrap_or(false)),
        );
        reconcile_preference(
            target,
            old,
            &mut source,
            "right_panel_width",
            json!(self
                .number("rightPanelWidth")
                .unwrap_or(288.)
                .clamp(220., 640.)
                .round()),
        );
        let tab = self
            .get("vorkspace.rightPanelTab")
            .filter(|s| ["files", "changes", "graph"].contains(s))
            .unwrap_or("files");
        reconcile_preference(target, old, &mut source, "right_panel_tab", json!(tab));
        let order = self
            .json("vorkspace.rightPanelTabOrder")
            .or_else(|| self.json("vorkspace.sidebarTabOrder"));
        let mut tabs = Vec::new();
        for tab in order
            .as_ref()
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .chain(["files", "changes", "graph"])
        {
            if ["files", "changes", "graph"].contains(&tab) && !tabs.contains(&tab) {
                tabs.push(tab);
            }
        }
        reconcile_preference(target, old, &mut source, "right_panel_tabs", json!(tabs));
        reconcile_preference(
            target,
            old,
            &mut source,
            "changes_view",
            json!(if self.get("vorkspace.changesView") == Some("tree") {
                "tree"
            } else {
                "list"
            }),
        );
        let collapsed = self
            .json("vorkspace:tab-groups:collapsed")
            .as_ref()
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        reconcile_preference(target, old, &mut source, "collapsed", json!(collapsed));
        let filters = self
            .json("vorkspace.sessionSidebarFilters")
            .unwrap_or(Value::Null);
        let time = filters["time"]
            .as_str()
            .filter(|s| ["all", "today", "7d", "30d"].contains(s))
            .unwrap_or("all");
        reconcile_preference(
            target,
            old,
            &mut source,
            "session_filters",
            json!({"hidden_providers":providers(&filters["hiddenHarnesses"]), "time":time, "status":{"working":filters["status"]["working"] == true, "needs_approval":filters["status"]["needsApproval"] == true, "done":filters["status"]["done"] == true}}),
        );
        next.insert("workspace".into(), json!(source));
        // Preserve exact strings for source concepts with no native equivalent.
        let mut metadata = target
            .get("desktop_preferences_metadata")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        for key in [
            "vorkspace.sessionFolders",
            "vorkspace.pinnedSessionsCollapsed",
            "vorkspace.reminderSessionsCollapsed",
            "vorkspace.projectProviderSettings.v1",
            "vorkspace.archivedProjects",
            "vorkspace.settingsSection",
            "vorkspace.quickComposerShortcut",
            "vorkspace.sidebarOpen",
            "vorkspace.sidebarLayout",
            "vorkspace.favoriteModels",
            "vorkspace.recentModels",
            "vorkspace.projectGroups",
            "vorkspace.projectGroupAssignments",
            "vorkspace.projectLocations",
            "vorkspace.projectSidebarTabs.v1",
            "vorkspace.projectRailOpen",
            "vorkspace.projectRailWidth",
            "vorkspace.projectRailPinned",
            "vorkspace.sidebarTabOrder",
            "vorkspace.transcriptZen",
            "vorkspace.projectNotifications.v1",
            "vorkspace.notificationProjects.v2",
            "vorkspace.newLayoutPromptDismissed",
            "vorkspace.linkedSessionSeen",
        ] {
            if let Some(value) = self.get(key) {
                metadata.insert(key.to_owned(), json!(value));
            }
        }
        target.insert("desktop_preferences_metadata".into(), json!(metadata));
    }

    fn flag(&self, key: &str) -> Option<bool> {
        self.get(&format!("vorkspace.{key}"))
            .map(|value| matches!(value, "1" | "true"))
    }
    pub(super) fn number(&self, key: &str) -> Option<f64> {
        self.get(&format!("vorkspace.{key}"))?
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
    }
}

fn retained_preferences(target: &Map<String, Value>) -> DesktopPreferences {
    DesktopPreferences {
        values: target
            .get("desktop_preferences_metadata")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
            .collect(),
        available: true,
    }
}

fn sidebar_tabs(value: &Value, allow_history: bool) -> Value {
    Value::Object(
        value
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(project, tab)| {
                let tab = tab.as_str()?;
                if project.is_empty()
                    || project == "~"
                    || !(["sessions", "files", "changes"].contains(&tab)
                        || allow_history && tab == "graph")
                {
                    return None;
                }
                Some((project.clone(), json!(tab)))
            })
            .collect(),
    )
}

fn sidebar_tab_order(value: &Value) -> Value {
    let mut tabs = Vec::new();
    for tab in value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .chain(["sessions", "files", "changes"])
    {
        if ["sessions", "files", "changes"].contains(&tab) && !tabs.contains(&tab) {
            tabs.push(tab);
        }
    }
    json!(tabs)
}

fn model_choice(provider: &str, model: &str) -> Option<Value> {
    crate::kind::Kind::parse(provider)?;
    if model
        .split_once(':')
        .is_some_and(|(prefix, _)| crate::kind::Kind::parse(prefix).is_some() && prefix != provider)
    {
        return None;
    }
    Some(json!({"provider":provider, "model":crate::session::native_model(model, provider)}))
}

pub(super) fn valid_hex(color: &str) -> bool {
    color.len() == 7 && color.starts_with('#') && color[1..].bytes().all(|b| b.is_ascii_hexdigit())
}
pub(super) fn providers(value: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for provider in value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|id| crate::kind::Kind::parse(id).is_some())
    {
        if !out.iter().any(|id| id == provider) {
            out.push(provider.to_owned());
        }
    }
    out
}
pub(super) fn native_models(value: &Value) -> Map<String, Value> {
    value
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(provider, model)| {
            crate::kind::Kind::parse(provider)?;
            Some((
                provider.clone(),
                json!(crate::session::native_model(model.as_str()?, provider)?),
            ))
        })
        .collect()
}

// Desktop chords use DOM physical codes. Native keymaps use GPUI key names.
fn shortcut(value: &str) -> Option<String> {
    let mut parts = value.split('+').collect::<Vec<_>>();
    let code = parts.pop()?;
    if parts.is_empty() {
        return None;
    }
    let mut modifiers = Vec::new();
    for part in parts {
        let native = match part {
            "Command" => "cmd",
            "Control" => "ctrl",
            "Option" => "alt",
            "Shift" => "shift",
            _ => return None,
        };
        if modifiers.contains(&native) {
            return None;
        }
        modifiers.push(native);
    }
    let key = match code {
        "Space" => "space",
        "Enter" | "NumpadEnter" => "enter",
        "Tab" => "tab",
        "Backspace" => "backspace",
        "Delete" => "delete",
        "ArrowUp" => "up",
        "ArrowDown" => "down",
        "ArrowLeft" => "left",
        "ArrowRight" => "right",
        "Home" => "home",
        "End" => "end",
        "PageUp" => "pageup",
        "PageDown" => "pagedown",
        "Backquote" => "`",
        "Backslash" => "\\",
        "BracketLeft" => "[",
        "BracketRight" => "]",
        "Comma" => ",",
        "Equal" => "=",
        "Minus" => "-",
        "Period" => ".",
        "Quote" => "'",
        "Semicolon" => ";",
        "Slash" => "/",
        _ if code
            .strip_prefix("Key")
            .is_some_and(|v| v.len() == 1 && v.bytes().all(|b| b.is_ascii_uppercase())) =>
        {
            &code[3..]
        }
        _ if code
            .strip_prefix("Digit")
            .is_some_and(|v| v.len() == 1 && v.bytes().all(|b| b.is_ascii_digit())) =>
        {
            &code[5..]
        }
        _ if code
            .strip_prefix('F')
            .and_then(|v| v.parse::<u8>().ok())
            .is_some_and(|n| (1..=24).contains(&n)) =>
        {
            code
        }
        _ => return None,
    };
    Some(format!(
        "{}-{}",
        modifiers.join("-"),
        key.to_ascii_lowercase()
    ))
}

#[cfg(test)]
mod tests;
