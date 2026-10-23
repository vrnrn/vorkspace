//! Desktop localStorage settings expressed in native workspace field names.
use super::{reconcile_preference, DesktopPreferences};
use serde_json::{json, Map, Value};

impl DesktopPreferences {
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
            ("quick_composer", "quickComposerEnabled", true),
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
                &["editor", "unified"][..],
                "editor",
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
            .filter(|(provider, _)| crate::provider::Kind::parse(provider).is_some())
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
            .filter(|tab| *tab == "favorites" || crate::provider::Kind::parse(tab).is_some())
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
        reconcile_preference(
            target,
            old,
            &mut source,
            "new_layout_prompt_dismissed",
            json!(self.flag("newLayoutPromptDismissed").unwrap_or(false)),
        );
        reconcile_preference(
            target,
            old,
            &mut source,
            "classic_layout",
            json!(!self.flag("insetLayout").unwrap_or(false)),
        );
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
        let metadata: Map<String, Value> = [
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
            "vorkspace.transcriptZen",
            "vorkspace.projectNotifications.v1",
            "vorkspace.notificationProjects.v2",
            "vorkspace.newLayoutPromptDismissed",
            "vorkspace.linkedSessionSeen",
        ]
        .into_iter()
        .filter_map(|key| self.get(key).map(|value| (key.to_owned(), json!(value))))
        .collect();
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

fn model_choice(provider: &str, model: &str) -> Option<Value> {
    crate::provider::Kind::parse(provider)?;
    if model.split_once(':').is_some_and(|(prefix, _)| {
        crate::provider::Kind::parse(prefix).is_some() && prefix != provider
    }) {
        return None;
    }
    Some(json!({"provider":provider, "model":crate::store::native_model(model, provider)}))
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
        .filter(|id| crate::provider::Kind::parse(id).is_some())
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
            crate::provider::Kind::parse(provider)?;
            Some((
                provider.clone(),
                json!(crate::store::native_model(model.as_str()?, provider)?),
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
mod tests {
    use super::*;

    fn source(entries: &[(&str, &str)]) -> DesktopPreferences {
        DesktopPreferences {
            available: true,
            values: entries
                .iter()
                .map(|(key, value)| ((*key).into(), (*value).into()))
                .collect(),
        }
    }

    #[test]
    fn voice_controls_read_existing_settings_and_preserve_later_native_choices() {
        let preferences = source(&[
            ("vorkspace.vox.enabled", "1"),
            ("vorkspace.vox.silent", "true"),
            (
                "vorkspace.vox.microphone.v1",
                r#"{"id":"saved-device","label":"USB microphone"}"#,
            ),
        ]);
        let mut workspace = json!({});
        preferences.apply_workspace(&mut workspace);
        assert_eq!(workspace["preferences"]["vox_enabled"], true);
        assert_eq!(workspace["preferences"]["vox_silent"], true);
        assert_eq!(
            workspace["preferences"]["vox_microphone"]["id"],
            "saved-device"
        );
        workspace["preferences"]["vox_enabled"] = json!(false);
        workspace["preferences"]["vox_microphone"] =
            json!({"id":"builtin","label":"built-in microphone"});
        preferences.apply_workspace(&mut workspace);
        assert_eq!(workspace["preferences"]["vox_enabled"], false);
        assert_eq!(workspace["preferences"]["vox_microphone"]["id"], "builtin");

        let mut empty = json!({});
        source(&[("vorkspace.vox.microphone.v1", r#"{"id":"","label":7}"#)])
            .apply_workspace(&mut empty);
        assert_eq!(empty["preferences"]["vox_enabled"], false);
        assert_eq!(empty["preferences"]["vox_microphone"]["id"], "builtin");
    }

    #[test]
    fn desktop_settings_and_sidebar_filters_use_native_names_units_and_enums() {
        let preferences = source(&[
            ("vorkspace.sounds", "0"),
            ("vorkspace.colorScheme", "light"),
            ("vorkspace.workspaceTheme", "tidepool"),
            ("vorkspace.insetLayout", "1"),
            ("vorkspace.sessionSidebarOpen", "0"),
            ("vorkspace.projectRailOpen", "1"),
            ("vorkspace.sidebarOpacity", "0.42"),
            ("vorkspace.uiScale", "1.4"),
            ("vorkspace.chatBackgroundSessionOpacity", "0.31"),
            (
                "vorkspace.rightPanelTabOrder",
                "[\"changes\",\"files\",\"changes\"]",
            ),
            ("vorkspace.changesView", "tree"),
            (
                "vorkspace.sessionSidebarFilters",
                r#"{"showArchived":true,"hiddenHarnesses":["codex","unknown","codex"],"time":"7d","status":{"needsApproval":true}}"#,
            ),
            (
                "vorkspace.defaultModels",
                r#"{"codex":"codex:gpt-6.1-sol","claude":"claude:opus-4.8"}"#,
            ),
            (
                "vorkspace.keybindingOverrides",
                r#"{"App: Search":{"shortcut":"Command+KeyJ"},"App: Settings":{"disabled":true}}"#,
            ),
        ]);
        let mut workspace = json!({});
        preferences.apply_workspace(&mut workspace);
        let settings = &workspace["preferences"];
        assert_eq!(settings["sounds"], false);
        assert_eq!(settings["theme"], "light");
        assert_eq!(settings["workspace_theme"], "tidepool");
        assert_eq!(settings["sidebar_opacity"], 42);
        assert_eq!(settings["ui_scale"], 140);
        assert_eq!(settings["chat_background_session"], 31);
        assert_eq!(settings["show_archived"], true);
        assert_eq!(settings["default_models"]["codex"], "gpt-6.1-sol");
        assert_eq!(settings["default_models"]["claude"], "claude-opus-4-8");
        assert_eq!(settings["keybindings"]["App: Search"], "cmd-j");
        assert!(settings["keybindings"]["App: Settings"].is_null());
        assert_eq!(workspace["classic_layout"], false);
        assert_eq!(workspace["sidebar_visible"], false);
        assert_eq!(
            workspace["right_panel_tabs"],
            json!(["changes", "files", "graph"])
        );
        assert_eq!(workspace["changes_view"], "tree");
        let filters: crate::session_filters::Filters =
            serde_json::from_value(workspace["session_filters"].clone()).unwrap();
        assert_eq!(filters.hidden_providers, ["codex"]);
        assert_eq!(filters.time, crate::session_filters::TimeFilter::Week);
        assert!(filters.status.needs_approval);
    }

    #[test]
    fn unrelated_desktop_map_edits_preserve_native_model_choices_and_added_keys() {
        let initial = source(&[(
            "vorkspace.defaultModels",
            r#"{"codex":"codex:gpt-old","claude":"claude:opus-4.8"}"#,
        )]);
        let mut workspace = json!({});
        initial.apply_workspace(&mut workspace);
        workspace["preferences"]["default_models"]["codex"] = json!("gpt-native");
        workspace["preferences"]["default_models"]["cursor"] = json!("native-added");
        workspace["preferences"]["sounds"] = json!(false);
        let changed = source(&[(
            "vorkspace.defaultModels",
            r#"{"codex":"codex:gpt-old","claude":"claude:sonnet-4.8"}"#,
        )]);
        changed.apply_workspace(&mut workspace);
        assert_eq!(
            workspace["preferences"]["default_models"]["codex"],
            "gpt-native"
        );
        assert_eq!(
            workspace["preferences"]["default_models"]["cursor"],
            "native-added"
        );
        assert_eq!(
            workspace["preferences"]["default_models"]["claude"],
            "claude-sonnet-4-8"
        );
        assert_eq!(workspace["preferences"]["sounds"], false);
    }

    #[test]
    fn project_backgrounds_keep_independent_opacities_and_native_overrides() {
        let source = source(&[(
            "vorkspace:project-chat-backgrounds",
            r#"{"/work":{"path":"/images/background.png","emptyOpacity":0.2,"sessionOpacity":0.4,"scope":"empty","effect":"ascii"}}"#,
        )]);
        let mut workspace = json!({});
        source.apply_workspace(&mut workspace);
        let appearance: crate::project_appearance::ProjectAppearance =
            serde_json::from_value(workspace["project_appearance"]["/work"].clone()).unwrap();
        let background = appearance.background.unwrap();
        assert!((background.empty_opacity - 0.2).abs() < 1e-6);
        assert!((background.session_opacity - 0.4).abs() < 1e-6);
        assert_eq!(background.scope, crate::project_appearance::Scope::Empty);
        assert_eq!(background.effect, crate::project_appearance::Effect::Ascii);
        workspace["project_appearance"]["/work"]["background"]["empty_opacity"] = json!(0.3);
        source.apply_workspace(&mut workspace);
        assert_eq!(
            workspace["project_appearance"]["/work"]["background"]["empty_opacity"],
            0.3
        );
    }

    #[test]
    fn desktop_chords_translate_without_interpreting_unknown_codes() {
        assert_eq!(
            shortcut("Command+Shift+KeyP").as_deref(),
            Some("cmd-shift-p")
        );
        assert_eq!(
            shortcut("Control+Option+ArrowLeft").as_deref(),
            Some("ctrl-alt-left")
        );
        assert!(shortcut("Command+Invalid").is_none());
    }

    #[test]
    fn source_picker_choices_and_file_mode_reconcile_and_unported_prefs_stay_exact() {
        let source = source(&[
            (
                "vorkspace.favoriteModels",
                r#"["claude:opus-4.8","codex:gpt-6.1-sol","claude:opus-4.8","unknown:model"]"#,
            ),
            (
                "vorkspace.recentModels",
                r#"[{"harness":"codex","model":"codex:gpt-6.1-sol"},{"harness":"claude","model":"claude:opus-4.8"}]"#,
            ),
            ("vorkspace.modelPickerTab", "codex"),
            ("vorkspace.fileViewMode", "source"),
            ("vorkspace.projectSidebarTabs.v1", r#"{"/work":"sessions"}"#),
        ]);
        let mut workspace = json!({});
        source.apply_workspace(&mut workspace);
        assert_eq!(
            workspace["preferences"]["favorite_models"],
            json!([
                {"provider":"claude", "model":"claude-opus-4-8"},
                {"provider":"codex", "model":"gpt-6.1-sol"}
            ])
        );
        assert_eq!(
            workspace["preferences"]["recent_models"][0]["provider"],
            "codex"
        );
        assert_eq!(workspace["preferences"]["model_picker_tab"], "codex");
        assert_eq!(workspace["preferences"]["file_view_mode"], "source");
        assert_eq!(
            workspace["desktop_preferences_metadata"]["vorkspace.projectSidebarTabs.v1"],
            r#"{"/work":"sessions"}"#
        );
        workspace["preferences"]["model_picker_tab"] = json!("favorites");
        workspace["preferences"]["file_view_mode"] = json!("preview");
        source.apply_workspace(&mut workspace);
        assert_eq!(workspace["preferences"]["model_picker_tab"], "favorites");
        assert_eq!(workspace["preferences"]["file_view_mode"], "preview");
    }

    #[test]
    fn absent_desktop_inset_setting_uses_the_desktop_classic_default() {
        let mut workspace = json!({});
        source(&[]).apply_workspace(&mut workspace);
        assert_eq!(workspace["classic_layout"], true);
        let mut isolated = json!({"classic_layout":false});
        DesktopPreferences::default().apply_workspace(&mut isolated);
        assert_eq!(isolated["classic_layout"], false);
    }
}
