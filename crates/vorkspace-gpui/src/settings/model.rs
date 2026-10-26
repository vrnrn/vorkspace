//! Port of src/features/settings/model/settings.ts: the pages, their rail
//! groups and the index Settings search jumps through.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Section {
    #[default]
    General,
    Connections,
    Appearance,
    Keybindings,
    Chat,
    Providers,
    Mcp,
    Skills,
    Bots,
    #[serde(alias = "inbox")]
    Connectors,
    Archive,
    Worktrees,
    Vox,
}

pub(crate) const GROUPS: [(&str, &[Section]); 2] = [
    (
        "App",
        &[
            Section::General,
            Section::Connections,
            Section::Appearance,
            Section::Keybindings,
            Section::Connectors,
            Section::Archive,
            Section::Worktrees,
        ],
    ),
    (
        "Agents",
        &[
            Section::Chat,
            Section::Providers,
            Section::Mcp,
            Section::Skills,
            Section::Bots,
            Section::Vox,
        ],
    ),
];

impl Section {
    pub(crate) const ALL: [Section; 13] = [
        Section::General,
        Section::Connections,
        Section::Appearance,
        Section::Keybindings,
        Section::Chat,
        Section::Providers,
        Section::Mcp,
        Section::Skills,
        Section::Bots,
        Section::Connectors,
        Section::Archive,
        Section::Worktrees,
        Section::Vox,
    ];
    pub(crate) fn id(self) -> &'static str {
        match self {
            Section::General => "general",
            Section::Vox => "vox",
            Section::Connections => "connections",
            Section::Appearance => "appearance",
            Section::Keybindings => "keybindings",
            Section::Chat => "chat",
            Section::Providers => "providers",
            Section::Mcp => "mcp",
            Section::Skills => "skills",
            Section::Bots => "bots",
            Section::Connectors => "connectors",
            Section::Archive => "archive",
            Section::Worktrees => "worktrees",
        }
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Vox => "Vox",
            Section::Connections => "Connections",
            Section::Appearance => "Appearance",
            Section::Keybindings => "Keybindings",
            Section::Chat => "Chat",
            Section::Providers => "Providers",
            Section::Mcp => "MCP",
            Section::Skills => "Skills",
            Section::Bots => "Bots",
            Section::Connectors => "Connectors",
            Section::Archive => "Archive",
            Section::Worktrees => "Worktrees",
        }
    }
    pub(crate) fn description(self) -> &'static str {
        match self {
            Section::Vox => "Voice control, microphone, and persistent conversation.",
            Section::General => {
                "The build you are running, how vorkspace reaches you, and the panels it shows."
            }
            Section::Connections => "Connect your machines and run agents remotely through SSH.",
            Section::Appearance => {
                "Theme, tint, translucency, interface scale, and conversation backgrounds."
            }
            Section::Keybindings => {
                "Every shortcut the workspace handles, from the app menu and the key handler."
            }
            Section::Chat => {
                "How transcripts read, what the composer does with a follow-up, how files save, and how diffs open."
            }
            Section::Providers => {
                "Provider accounts, agent CLIs vorkspace can drive, and the model new sessions start with."
            }
            Section::Mcp => "Find MCP servers across providers and manage their connections.",
            Section::Skills => {
                "Discover and manage file skills from project, personal, and harness folders."
            }
            Section::Bots => "The resident agent beside your tabs, and which projects have one.",
            Section::Connectors => {
                "Manage GitHub, GitLab and Jira connectors and notification preferences for each project."
            }
            Section::Archive => "Projects and conversations you have archived.",
            Section::Worktrees => "Manage additional worktrees for each project.",
        }
    }
    fn keywords(self) -> &'static str {
        match self {
            Section::General => {
                "version update sounds notifications notes rail clipboard copy paste"
            }
            Section::Vox => "voice microphone speech conversation memory silent",
            Section::Connections => "ssh remote host machine server environment always on",
            Section::Appearance => {
                "theme dark light color accent glass blur zoom scale wallpaper rail sidebar"
            }
            Section::Keybindings => "shortcut hotkey keyboard binding",
            Section::Chat => {
                "transcript composer prompt message diff review layout format save editor"
            }
            Section::Providers => {
                "account sign in login model harness claude codex gemini cli default hooks"
            }
            Section::Mcp => {
                "tools servers connections oauth authenticate login claude codex antigravity pi omp"
            }
            Section::Skills => "skill instructions prompt",
            Section::Bots => "bot resident agent mascot claim project title bar",
            Section::Connectors => "github gitlab jira atlassian connect token integration",
            Section::Archive => "archived restore delete hidden",
            Section::Worktrees => "git branch worktree working copy project create delete",
        }
    }
    /// SettingsRail.tsx's SECTION_ICONS.
    pub(crate) fn icon(self) -> &'static str {
        match self {
            Section::General => "sliders",
            Section::Vox => "mic",
            Section::Connections => "internet",
            Section::Appearance => "palette",
            Section::Keybindings => "keyboard",
            Section::Chat => "message-square",
            Section::Providers => "bot",
            Section::Mcp => "globe",
            Section::Skills => "sparkles",
            Section::Bots => "ghost",
            Section::Connectors => "globe",
            Section::Archive => "archive",
            Section::Worktrees => "folder-tree",
        }
    }
}

/// SETTINGS_INDEX: (id, section, label, keywords).
pub(crate) const INDEX: &[(&str, Section, &str, &str)] = &[
    (
        "remote-machines",
        Section::Connections,
        "Your machines",
        "ssh remote connect host server environment",
    ),
    (
        "mcp-servers",
        Section::Mcp,
        "MCP servers",
        "claude tools connections oauth authenticate login add remove",
    ),
    (
        "bots-enabled",
        Section::Bots,
        "Show bots",
        "bot agent rail hide",
    ),
    (
        "rail-bots-pinned",
        Section::Bots,
        "Pin bots to the icon rail",
        "bot rail compact collapsed icons top divider project picker",
    ),
    (
        "bot-list",
        Section::Bots,
        "Your bots",
        "bot reset soul name projects sessions sidebar visibility hidden show",
    ),
    (
        "project-worktrees",
        Section::Worktrees,
        "Project worktrees",
        "git branch working copy create delete manage",
    ),
    (
        "update",
        Section::General,
        "Version",
        "update upgrade release what's new build changelog",
    ),
    (
        "sounds",
        Section::General,
        "Sounds",
        "audio cue chime mute volume",
    ),
    (
        "notifications",
        Section::General,
        "Notifications",
        "notify alert toast permission reminder background",
    ),
    (
        "notes",
        Section::General,
        "Notes",
        "notebook markdown rail scratchpad",
    ),
    (
        "quick-composer",
        Section::General,
        "Quick composer",
        "spotlight global shortcut hotkey floating prompt anywhere",
    ),
    (
        "bot-menu-bar-icon",
        Section::General,
        "Menu bar icon",
        "macos status tray quick composer quit",
    ),
    (
        "clipboard-history",
        Section::General,
        "Clipboard",
        "copy paste text screenshot image clipboard memory menu bar",
    ),
    (
        "file-tabs",
        Section::General,
        "File tabs",
        "editor open top workspace normal session pane beside chat",
    ),
    (
        "file-word-wrap",
        Section::General,
        "Word wrap",
        "files editor long lines horizontal scroll wrapping",
    ),
    (
        "tab-animations",
        Section::General,
        "Tab animations",
        "motion open close resize transition",
    ),
    (
        "theme",
        Section::Appearance,
        "Theme",
        "dark light system appearance mode",
    ),
    (
        "workspace-theme",
        Section::Appearance,
        "Workspace theme",
        "workspace theme color colour text accent syntax scheme preset harbor moss ember orchid",
    ),
    (
        "accent-color",
        Section::Appearance,
        "Accent color",
        "highlight bubble send button tint",
    ),
    (
        "diff-colors",
        Section::Appearance,
        "Diff colors",
        "colorblind color blind accessibility added removed red green blue orange high contrast changes",
    ),
    ("hue", Section::Appearance, "Hue", "tint color chrome"),
    (
        "saturation",
        Section::Appearance,
        "Saturation",
        "tint color neutral grey gray",
    ),
    (
        "dark-lightness",
        Section::Appearance,
        "Dark-mode lightness",
        "black brightness contrast background",
    ),
    (
        "sidebar-opacity",
        Section::Appearance,
        "Sidebar opacity",
        "window opacity glass translucent transparency vibrancy",
    ),
    (
        "blur",
        Section::Appearance,
        "Blur radius",
        "window sidebar glass translucent transparency background radius",
    ),
    (
        "main-pane-glass",
        Section::Appearance,
        "Main pane glass",
        "translucent transparency body window",
    ),
    (
        "interface-scale",
        Section::Appearance,
        "Interface scale",
        "zoom font size bigger smaller ui",
    ),
    (
        "collapsed-project-rail",
        Section::Appearance,
        "Collapsed project rail",
        "sidebar compact icons hidden navigation layout",
    ),
    (
        "show-excluded-files",
        Section::Appearance,
        "Show excluded files",
        "explorer gitignore ignored hidden files tree",
    ),
    (
        "chat-background",
        Section::Appearance,
        "Chat background",
        "wallpaper image picture opacity backdrop",
    ),
    (
        "transcript-layout",
        Section::Chat,
        "Transcript layout",
        "full width chat bubble message",
    ),
    (
        "anchor-prompts",
        Section::Chat,
        "Anchor prompts to top",
        "scroll position sticky message",
    ),
    (
        "paced-reveal",
        Section::Chat,
        "Smooth text reveal",
        "streaming pacing immediate latency performance animation",
    ),
    (
        "follow-up",
        Section::Chat,
        "Follow-up behavior",
        "queue steer interrupt send while running",
    ),
    (
        "model-controls",
        Section::Chat,
        "Model controls",
        "effort thinking reasoning fast service tier model picker composer",
    ),
    (
        "composer-autocorrect",
        Section::Chat,
        "Autocorrect",
        "spelling spell check autocorrect typo macos composer",
    ),
    (
        "format-on-save",
        Section::Chat,
        "Format on save",
        "prettier quotes editor save format",
    ),
    (
        "diff-view",
        Section::Chat,
        "Diff view",
        "unified side by side split review changes working tree",
    ),
    (
        "agent-clis",
        Section::Providers,
        "Agent CLIs",
        "claude codex pi omp antigravity binary path",
    ),
    (
        "provider-accounts",
        Section::Providers,
        "Provider accounts",
        "account sign in login rename remove delete credentials profile usage limit quota exhausted",
    ),
    (
        "show-remaining-usage",
        Section::Providers,
        "Show remaining usage",
        "usage limit meter bar left used quota percent",
    ),
    (
        "mask-emails",
        Section::Providers,
        "Mask account emails",
        "email privacy blur hide screenshot account",
    ),
    (
        "claude-hooks",
        Section::Providers,
        "Claude Code hooks",
        "pretooluse settings.json block command notification",
    ),
    (
        "idle-agents",
        Section::Providers,
        "Keep idle agents ready",
        "memory ram cpu performance process warm park background",
    ),
    (
        "project-notifications",
        Section::Connectors,
        "Project notifications",
        "mute resume sounds banners reminders categories",
    ),
    (
        "github",
        Section::Connectors,
        "GitHub",
        "gh cli connect pull request sign in",
    ),
    (
        "gitlab",
        Section::Connectors,
        "GitLab",
        "token self-managed merge request connect",
    ),
    (
        "jira",
        Section::Connectors,
        "Jira",
        "atlassian cloud site email api token issues projects connect",
    ),
    (
        "show-archived",
        Section::Archive,
        "Show archived in the sidebar",
        "hidden conversations list",
    ),
];

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SearchResult {
    pub section: Section,
    pub setting: Option<&'static str>,
    pub label: &'static str,
}

fn score(needle: &str, label: &str, keywords: &str) -> Option<f32> {
    let lower = label.to_lowercase();
    if lower.starts_with(needle) {
        Some(0.)
    } else if lower.contains(needle) {
        Some(1.)
    } else if keywords.to_lowercase().contains(needle) {
        Some(2.)
    } else {
        None
    }
}

/// searchSettings: individual settings first, then whole sections.
pub(crate) fn search(query: &str) -> Vec<SearchResult> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(f32, SearchResult)> = Vec::new();
    for &(id, section, label, keywords) in INDEX {
        if let Some(s) = score(&needle, label, keywords) {
            scored.push((
                s,
                SearchResult {
                    section,
                    setting: Some(id),
                    label,
                },
            ));
        }
    }
    for section in Section::ALL {
        let keywords = format!("{} {}", section.description(), section.keywords());
        if let Some(s) = score(&needle, section.label(), &keywords) {
            scored.push((
                s + 0.5,
                SearchResult {
                    section,
                    setting: None,
                    label: section.label(),
                },
            ));
        }
    }
    scored.sort_by(|a, b| {
        a.0.total_cmp(&b.0)
            .then_with(|| a.1.label.to_lowercase().cmp(&b.1.label.to_lowercase()))
    });
    scored.into_iter().take(8).map(|(_, r)| r).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_groups_place_workspace_pages_in_app_without_changing_identity() {
        assert_eq!(
            GROUPS.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
            ["App", "Agents"]
        );
        assert_eq!(
            GROUPS[0].1,
            &[
                Section::General,
                Section::Connections,
                Section::Appearance,
                Section::Keybindings,
                Section::Connectors,
                Section::Archive,
                Section::Worktrees,
            ]
        );
        let pages: Vec<_> = GROUPS
            .iter()
            .flat_map(|(_, pages)| pages.iter().copied())
            .collect();
        assert_eq!(pages.len(), Section::ALL.len());
        for section in Section::ALL {
            assert_eq!(pages.iter().filter(|page| **page == section).count(), 1);
        }
        assert_eq!(Section::Connections.id(), "connections");
        assert_eq!(Section::Connectors.id(), "connectors");
        assert!(Section::Connections.description().contains("SSH"));
        assert_eq!(
            serde_json::from_str::<Section>(r#""inbox""#).unwrap(),
            Section::Connectors
        );
    }
    #[test]
    fn every_settings_navigation_icon_renders_visible_pixels_at_its_display_size() {
        use gpui::{AssetSource, DevicePixels, SvgRenderer, SvgSize, size};
        let assets = std::sync::Arc::new(crate::assets::Assets);
        let renderer = SvgRenderer::new(assets.clone());
        for section in Section::ALL {
            let key = section.icon();
            let bytes = assets.load(key).unwrap().unwrap_or_else(|| {
                panic!("{} navigation icon {key} is not bundled", section.label())
            });
            let parsed = renderer.parse_svg(&bytes).unwrap();
            let image = renderer
                .render_parsed(
                    &parsed,
                    SvgSize::ExactSize(size(DevicePixels(16), DevicePixels(16))),
                )
                .unwrap();
            assert!(
                image
                    .as_bytes(0)
                    .unwrap()
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| pixel[3] != 0),
                "{} navigation icon {key} renders no visible pixels",
                section.label()
            );
        }
    }
    #[test]
    fn rows_win_their_own_name_and_shared_appearance_rows_are_searchable() {
        let results = search("theme");
        assert_eq!(results[0].setting, Some("theme"));
        assert_eq!(results[1].setting, Some("workspace-theme"));
        assert!(search("hue").iter().any(|r| r.setting == Some("hue")));
        assert!(
            search("workspace theme")
                .iter()
                .any(|r| r.setting == Some("workspace-theme"))
        );
        let pages = search("archive");
        assert!(
            pages
                .iter()
                .any(|r| r.section == Section::Archive && r.setting.is_none())
        );
    }
}
