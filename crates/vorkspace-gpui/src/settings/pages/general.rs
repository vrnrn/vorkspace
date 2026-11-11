//! GeneralPage and UpdateRow.
use crate::settings::Preferences;

use crate::settings::{SettingsView, widgets::secondary_button};
use gpui::{AnyElement, Context, FontWeight, div, prelude::*, px};
use vorkspace_ui::theme::*;

impl SettingsView {
    pub(super) fn general_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let p = self.prefs.clone();
        let quick_hint = super::keybindings::command_shortcut(&p, "App: Quick Composer")
            .map(|shortcut| format!(
                "Press {shortcut} from any app. Return starts a session in the background and ⌘Return opens it. Change the shortcut in Keybindings."
            ))
            .unwrap_or_else(|| "Set a shortcut for Quick Composer in Keybindings.".into());
        let alerts = self.general_alert_rows(&p, cx);
        let mut workspace = self.general_workspace_rows(&p, cx);
        self.general_workspace_controls(&p, quick_hint, &mut workspace, cx);
        let about = self.general_about_rows(cx);
        let mut groups = vec![
            self.group(None, true, "Alerts", None, None, alerts),
            self.group(None, false, "Workspace", None, None, workspace),
        ];
        {
            groups.push(self.clipboard_group(cx));
            groups.push(self.dictation_group(cx));
        }
        groups.push(self.group(None, false, "About", None, None, about));
        groups
    }

    fn general_workspace_controls(
        &self,
        p: &Preferences,
        quick_hint: String,
        workspace: &mut Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) {
        {
            workspace.push(
                self.row(
                    Some("bot-menu-bar-icon"),
                    false,
                    "Menu bar icon",
                    Some(
                        "Access clipboard history and recording controls from the macOS menu bar."
                            .into(),
                    ),
                    vec![self.toggle(
                        "bot-menu-bar-icon",
                        "Menu bar icon",
                        p.bot_menu_bar_icon,
                        false,
                        cx,
                        |this, on, cx| this.update(cx, |p| p.bot_menu_bar_icon = on),
                    )],
                ),
            );
            workspace.push(self.row(
                Some("quick-composer"),
                false,
                "Quick composer",
                Some(quick_hint.into()),
                {
                    let mut controls = Vec::new();
                    if let Some(error) = &self.ctx.quick_error {
                        controls.push(super::super::widgets::note(error.clone()));
                    }
                    controls.push(self.toggle(
                        "quick-composer",
                        "Quick composer",
                        p.quick_composer,
                        false,
                        cx,
                        |this, on, cx| this.update(cx, |p| p.quick_composer = on),
                    ));
                    controls
                },
            ));
        }
    }

    fn general_workspace_rows(&self, p: &Preferences, cx: &mut Context<Self>) -> Vec<AnyElement> {
        vec![
            self.row(
                Some("file-tabs"),
                true,
                "File tabs",
                Some("Choose where files open.".into()),
                vec![self.segmented(
                    "file-tabs",
                    "File tabs",
                    &p.file_tabs,
                    &[("pane", "Beside chat"), ("workspace", "Top bar")],
                    cx,
                    |this, v, cx| this.update(cx, |p| p.file_tabs = v.into()),
                )],
            ),
            self.row(
                Some("file-word-wrap"),
                false,
                "Word wrap",
                Some("Wrap long lines to fit the editor.".into()),
                vec![self.toggle(
                    "file-word-wrap",
                    "Word wrap",
                    p.file_word_wrap,
                    false,
                    cx,
                    |this, on, cx| this.update(cx, |p| p.file_word_wrap = on),
                )],
            ),
            self.row(
                Some("tab-animations"),
                false,
                "Tab animations",
                Some("Animate tabs as they open and close.".into()),
                vec![self.toggle(
                    "tab-animations",
                    "Tab animations",
                    p.tab_animations,
                    false,
                    cx,
                    |this, on, cx| this.update(cx, |p| p.tab_animations = on),
                )],
            ),
            self.row(
                Some("working-agents"),
                false,
                "Working agents",
                Some("Show running sessions and unseen replies above Subscriptions.".into()),
                vec![self.toggle(
                    "working-agents",
                    "Working agents",
                    p.working_agents,
                    false,
                    cx,
                    |this, on, cx| this.update(cx, |p| p.working_agents = on),
                )],
            ),
        ]
    }

    fn general_about_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let version = env!("CARGO_PKG_VERSION");
        vec![
            self.row(
                Some("update"),
                true,
                div()
                    .flex()
                    .items_baseline()
                    .gap_2()
                    .child("Version")
                    .child(
                        div()
                            .font_family(vorkspace_ui::fonts::MONO)
                            .text_size(px(12.))
                            .font_weight(FontWeight::NORMAL)
                            .text_color(content(0.45))
                            .child(version),
                    ),
                Some("View release notes and check for updates.".into()),
                vec![
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            secondary_button("whats-new", "What's new", false, false)
                                .child("What's new")
                                .on_click(cx.listener(|_, _, _, cx| {
                                    cx.emit(crate::settings::SettingsEvent::WhatsNew)
                                })),
                        )
                        .child(
                            secondary_button(
                                "check-harness-updates",
                                "Check harness updates",
                                false,
                                false,
                            )
                            .child("Check harness updates")
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(crate::settings::SettingsEvent::CheckHarnessUpdates)
                            })),
                        )
                        .child(
                            secondary_button("check-updates", "Check for updates", false, false)
                                .child(icon("refresh-cw", 14.).text_color(content(0.7)))
                                .child("Check for updates")
                                .on_click(|_, _, cx| vorkspace_macos::updater::check(cx)),
                        )
                        .into_any_element(),
                ],
            ),
        ]
    }

    fn general_alert_rows(&self, p: &Preferences, cx: &mut Context<Self>) -> Vec<AnyElement> {
        vec![
            self.row(
                Some("sounds"),
                true,
                "Sounds",
                Some("Play sounds for activity, completed turns, and updates.".into()),
                vec![
                    self.toggle("sounds", "Sounds", p.sounds, false, cx, |this, on, cx| {
                        this.update(cx, |p| p.sounds = on)
                    }),
                ],
            ),
            self.row(
                Some("notifications"),
                false,
                "Notifications",
                Some("Alert when reminders are due or agents finish or need input.".into()),
                {
                    let mut controls = self.notification_permission_controls(cx);
                    controls.push(self.toggle(
                        "notifications",
                        "Notifications",
                        p.notifications,
                        false,
                        cx,
                        |this, on, cx| {
                            this.update(cx, |p| p.notifications = on);
                            if on {
                                this.probe_notification_permission(true, cx);
                            }
                        },
                    ));
                    controls
                },
            ),
        ]
    }
}
