//! React's command names, defaults, scopes, filter and shortcut recorder.

use crate::settings::{Preferences, SettingsView};
use gpui::{AnyElement, Context, Focusable, FontWeight, Role, Window, div, prelude::*, px};
use vorkspace_ui::radius;
use vorkspace_ui::theme::*;
#[derive(Clone, Copy)]
pub(crate) struct Binding {
    pub command: &'static str,
    pub keys: &'static str,
    pub when: &'static str,
    pub fixed: bool,
}
pub(crate) const BINDINGS: &[Binding] = &[
    Binding {
        command: "App: Settings",
        keys: "⌘,",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "App: Search",
        keys: "⌘K",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "App: Go to File",
        keys: "⌘P",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "App: Command Palette",
        keys: "⌘⇧P",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "App: Find in Files",
        keys: "⌘⇧F",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "App: Open Project",
        keys: "⌘O",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "App: New Claude Session",
        keys: "⌘⇧N",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "App: Dictation",
        keys: "Fn",
        when: "Anywhere",
        fixed: false,
    },
    Binding {
        command: "App: Vox",
        keys: "⌃⌥Space",
        when: "Anywhere",
        fixed: false,
    },
    Binding {
        command: "App: Quick Composer",
        keys: "⌘⇧Space",
        when: "Anywhere",
        fixed: false,
    },
    Binding {
        command: "App: Toggle Sidebar",
        keys: "⌘B",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "App: Toggle Session Sidebar",
        keys: "⌘⇧B",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "App: Switch Model",
        keys: "⌘.",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "App: Toggle Bot",
        keys: "⌘I",
        when: "Project with a bot",
        fixed: false,
    },
    Binding {
        command: "Composer: Toggle Workspace",
        keys: "⌘⇧G",
        when: "Draft session composer",
        fixed: false,
    },
    Binding {
        command: "View: Reload",
        keys: "⌘⇧R",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "View: Zoom In",
        keys: "⌘+",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "View: Zoom Out",
        keys: "⌘-",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "View: Reset Zoom",
        keys: "⌘0",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Tab: New",
        keys: "⌘T",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Tab: Close Others",
        keys: "⌘⌥T",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Tab: Close All",
        keys: "⌘⇧W",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Tab: Next",
        keys: "⌘⇧]",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Tab: Previous",
        keys: "⌘⇧[",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Tab: Cycle Next",
        keys: "⌃Tab",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Tab: Cycle Previous",
        keys: "⌃⇧Tab",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Tab: Back",
        keys: "⌘[",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Tab: Forward",
        keys: "⌘]",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Tab: Activate 1–8",
        keys: "⌘1 … ⌘8",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Tab: Activate Last",
        keys: "⌘9",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Session: Archive",
        keys: "⌘⇧A",
        when: "sessionFocus && !overlay",
        fixed: false,
    },
    Binding {
        command: "Session: Stop",
        keys: "Esc",
        when: "sessionBusy && !terminalFocus",
        fixed: true,
    },
    Binding {
        command: "Session: Previous",
        keys: "⌘⇧↑",
        when: "!overlay && (!textFocus || emptyComposer)",
        fixed: false,
    },
    Binding {
        command: "Session: Next",
        keys: "⌘⇧↓",
        when: "!overlay && (!textFocus || emptyComposer)",
        fixed: false,
    },
    Binding {
        command: "Session: Previous in Current Tab",
        keys: "⌘↑",
        when: "!overlay && (!textFocus || emptyComposer)",
        fixed: false,
    },
    Binding {
        command: "Session: Next in Current Tab",
        keys: "⌘↓",
        when: "!overlay && (!textFocus || emptyComposer)",
        fixed: false,
    },
    Binding {
        command: "Project: Previous",
        keys: "⌘⇧←",
        when: "!overlay && (!textFocus || emptyComposer)",
        fixed: false,
    },
    Binding {
        command: "Project: Next",
        keys: "⌘⇧→",
        when: "!overlay && (!textFocus || emptyComposer)",
        fixed: false,
    },
    Binding {
        command: "Pane: Close",
        keys: "⌘W",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Pane: Split Right",
        keys: "⌘D",
        when: "!editorFocus",
        fixed: false,
    },
    Binding {
        command: "Pane: Split Down",
        keys: "⌘⇧D",
        when: "!editorFocus",
        fixed: false,
    },
    Binding {
        command: "Pane: Focus Left",
        keys: "⌘⌥←",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Pane: Focus Right",
        keys: "⌘⌥→",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Pane: Focus Up",
        keys: "⌘⌥↑",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Pane: Focus Down",
        keys: "⌘⌥↓",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Terminal: New",
        keys: "⌘`",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Terminal: New Tab",
        keys: "⌘⇧`",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Terminal: Toggle Dock",
        keys: "⌘J",
        when: "Always",
        fixed: false,
    },
    Binding {
        command: "Editor: Find",
        keys: "⌘F",
        when: "editorFocus",
        fixed: false,
    },
    Binding {
        command: "Editor: Replace",
        keys: "⌘⌥F",
        when: "editorFocus",
        fixed: false,
    },
];
/// A command's shortcut as shown, honoring the user's rebinding; `None`
/// once they have disabled it.
pub(crate) fn command_shortcut(p: &Preferences, command: &str) -> Option<String> {
    match p.keybindings.get(command) {
        Some(Some(s)) => Some(shortcut_label(s)),
        Some(None) => None,
        None => {
            let keys = BINDINGS.iter().find(|r| r.command == command)?.keys;
            Some(keys.into())
        }
    }
}
pub(crate) fn shortcut_label(shortcut: &str) -> String {
    if shortcut == "fn" {
        return "Fn / Globe".into();
    }
    if shortcut == "right-alt" {
        return "Right Option".into();
    }
    let parts: Vec<_> = shortcut.split('-').collect();
    let mut label = String::new();
    for part in parts {
        label.push_str(&match part {
            "cmd" => "⌘".into(),
            "ctrl" => "⌃".into(),
            "alt" => "⌥".into(),
            "shift" => "⇧".into(),
            "space" => "Space".into(),
            "up" => "↑".into(),
            "down" => "↓".into(),
            "left" => "←".into(),
            "right" => "→".into(),
            "tab" => "Tab".into(),
            key => key.to_uppercase(),
        });
    }
    label
}
impl SettingsView {
    pub(super) fn keybindings_page(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let pane_width = self.scroll.bounds().size.width.as_f32();
        let narrow =
            window.viewport_size().width < px(800.) || (pane_width > 0. && pane_width < 580.);
        let query = self.keybinding_query.trim().to_lowercase();
        let rows: Vec<_> = BINDINGS
            .iter()
            .filter(|r| {
                format!("{} {} {}", r.command, self.binding_label(r), r.when)
                    .to_lowercase()
                    .contains(&query)
            })
            .copied()
            .collect();
        let action = self.keybinding_filter_action(&rows, cx);
        let mut action = Some(action);
        let mut children = vec![if narrow {
            div()
                .px_4()
                .py_2()
                .children(action.take())
                .into_any_element()
        } else {
            div()
                .flex()
                .items_center()
                .border_b_1()
                .border_color(content(0.1))
                .bg(content(0.05))
                .px_4()
                .py_2()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(content(0.5))
                .child(div().min_w_0().flex_1().child("Command"))
                .child(div().w(px(160.)).flex_shrink_0().child("Keybinding"))
                .child(div().w(px(150.)).flex_shrink_0().child("When"))
                .into_any_element()
        }];
        if rows.is_empty() {
            children.push(super::super::widgets::note("No matching bindings"));
        }
        // Commands are named "Area: Action". The area becomes a header the
        // first time it appears so each row only has to say what it does.
        let mut area = "";
        for row in rows {
            let command = row.command;
            let (row_area, name) = command.split_once(": ").unwrap_or(("", command));
            if row_area != area && !row_area.is_empty() {
                area = row_area;
                children.push(
                    div()
                        .h(px(32.))
                        .flex()
                        .items_end()
                        .pb(px(6.))
                        .px_4()
                        .border_b_1()
                        .border_color(content(0.05))
                        .text_size(px(11.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(content(0.45))
                        .child(row_area)
                        .into_any_element(),
                );
            }
            children.push(self.keybinding_row(row, name, narrow, cx));
        }
        if let Some(error) = &self.recording_error {
            children.push(
                div()
                    .px_4()
                    .py_2()
                    .text_size(px(11.))
                    .text_color(vorkspace_ui::theme::danger())
                    .child(error.clone())
                    .into_any_element(),
            );
        }
        vec![self.group(None,true,"Shortcuts", Some("Click a shortcut to record new keys. Press Delete while recording to disable it.".into()),action,children)]
    }

    fn keybinding_filter_action(&self, rows: &[Binding], cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .items_center()
            .flex_wrap()
            .max_w_full()
            .gap_3()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(content(0.4))
                    .child(format!(
                        "{} {}",
                        rows.len(),
                        if rows.len() == 1 {
                            "binding"
                        } else {
                            "bindings"
                        }
                    )),
            )
            .child(
                crate::controls::page::search_frame(&self.keybinding_filter.focus_handle(cx))
                    .w(px(176.))
                    .max_w_full()
                    .child(icon("search-01", 14.))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .child(self.keybinding_filter.clone()),
                    ),
            )
            .into_any_element()
    }

    fn keybinding_row(
        &self,
        row: Binding,
        name: &'static str,
        narrow: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let command = row.command;
        let recording = self.recording.as_deref() == Some(command);
        let modified = self.prefs.keybindings.contains_key(command);
        let disabled = self.prefs.keybindings.get(command) == Some(&None);
        let label = if recording {
            if self.recording_preview.is_empty() {
                "Record…".into()
            } else {
                self.recording_preview.clone()
            }
        } else {
            self.binding_label(&row)
        };
        div()
            .flex()
            .map(|d| {
                if narrow {
                    d.flex_col().items_start().gap_1().py_2()
                } else {
                    d.h(px(36.)).items_center()
                }
            })
            .border_b_1()
            .border_color(content(0.05))
            .px_4()
            .text_size(px(12.))
            .child(
                div()
                    .min_w_0()
                    .when(narrow, |d| d.w_full())
                    .when(!narrow, |d| d.flex_1())
                    .truncate()
                    .text_color(content(if disabled { 0.45 } else { 1. }))
                    .child(name),
            )
            .child(
                div()
                    .w(px(160.))
                    .max_w_full()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .child(Self::shortcut_recorder(row, recording, disabled, label, cx))
                    .when(modified, |d| {
                        d.child(
                            div()
                                .id(gpui::SharedString::from(format!("reset-{command}")))
                                .debug_selector(move || format!("reset-shortcut-{command}"))
                                .role(Role::Button)
                                .map(vorkspace_ui::theme::tab_reachable)
                                .aria_label(format!("Reset {command} shortcut"))
                                .p_1()
                                .rounded(px(radius::CONTROL))
                                .cursor_pointer()
                                .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.1)))
                                .child(icon("rotate-ccw", 14.))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.update(cx, |p| {
                                        p.keybindings.remove(command);
                                    })
                                })),
                        )
                    }),
            )
            .when(!narrow, |d| {
                d.child(
                    div()
                        .w(px(150.))
                        .flex_shrink_0()
                        .truncate()
                        .text_size(px(12.))
                        .text_color(content(0.45))
                        .child(row.when),
                )
            })
            .into_any_element()
    }

    fn shortcut_recorder(
        row: Binding,
        recording: bool,
        disabled: bool,
        label: String,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let command = row.command;
        div()
            .id(gpui::SharedString::from(format!("shortcut-{command}")))
            .debug_selector(move || format!("shortcut-{command}"))
            .role(if row.fixed { Role::Label } else { Role::Button })
            .aria_label(format!("Change {command} shortcut"))
            .aria_description(format!("Available {}", row.when))
            .h(px(24.))
            .w(px(112.))
            .flex()
            .items_center()
            .truncate()
            .rounded(px(radius::CONTROL))
            .border_1()
            .border_color(if recording {
                accent_color()
            } else {
                content(0.15)
            })
            .px(px(6.))
            // The system font draws ⌘ ⇧ ⌥ at text size;
            // Obel Mono falls back to a tiny glyph.
            .text_size(px(12.))
            .font_features(crate::panel_kit::tabular())
            .text_color(content(if disabled {
                0.35
            } else if row.fixed {
                0.6
            } else {
                0.8
            }))
            .child(label)
            .when(!row.fixed, |d| {
                d.map(vorkspace_ui::theme::tab_reachable)
                    .cursor_pointer()
                    .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.1)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.recording = Some(command.into());
                        this.recording_preview.clear();
                        this.recording_error = None;
                        window.focus(&this.recorder_focus, cx);
                        cx.notify();
                    }))
            })
    }

    fn binding_label(&self, row: &Binding) -> String {
        command_shortcut(&self.prefs, row.command).unwrap_or_else(|| "Disabled".into())
    }
    pub(in crate::settings) fn record_key(
        &mut self,
        key: &gpui::Keystroke,
        cx: &mut Context<Self>,
    ) {
        let Some(command) = self.recording.clone() else {
            return;
        };
        cx.stop_propagation();
        if key.key == "escape" || key.key == "tab" {
            self.recording = None;
            cx.notify();
            return;
        }
        if matches!(
            key.key.as_str(),
            "platform" | "control" | "alt" | "shift" | "function"
        ) {
            return;
        }
        let bare = !key.modifiers.platform
            && !key.modifiers.control
            && !key.modifiers.alt
            && !key.modifiers.shift;
        if bare && matches!(key.key.as_str(), "backspace" | "delete") {
            self.recording = None;
            self.update(cx, |p| {
                p.keybindings.insert(command, None);
            });
            return;
        }
        let shortcut = key.unparse();
        self.recording_preview = shortcut_label(&shortcut);
        if bare || (!key.modifiers.platform && !key.modifiers.control && !key.modifiers.alt) {
            self.recording_error = Some("Use Command, Control, or Option with a key.".into());
            cx.notify();
            return;
        }
        if command == "Tab: Activate 1–8"
            && !matches!(
                key.key.as_str(),
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8"
            )
        {
            self.recording_error = Some("Tab: Activate 1–8 needs a number key from 1 to 8".into());
            cx.notify();
            return;
        }
        if let Some(other) = BINDINGS.iter().find(|r| {
            r.command != command
                && (default_shortcuts(r).contains(&shortcut)
                    || self
                        .prefs
                        .keybindings
                        .get(r.command)
                        .and_then(|s| s.as_ref())
                        == Some(&shortcut))
        }) {
            self.recording_error = Some(format!("Already used by {}", other.command));
            cx.notify();
            return;
        }
        self.recording = None;
        self.recording_error = None;
        self.update(cx, |p| {
            p.keybindings.insert(command, Some(shortcut));
        });
    }
}

fn default_shortcuts(row: &Binding) -> Vec<String> {
    let shortcut = row
        .keys
        .replace('⌘', "cmd-")
        .replace('⌃', "ctrl-")
        .replace('⌥', "alt-")
        .replace('⇧', "shift-")
        .replace('↑', "up")
        .replace('↓', "down")
        .replace('←', "left")
        .replace('→', "right")
        .replace('+', "=")
        .to_lowercase();
    let chords = if shortcut.contains('…') {
        let prefix = shortcut.split('1').next().unwrap_or("");
        (1..=8).map(|i| format!("{prefix}{i}")).collect()
    } else {
        vec![shortcut]
    };
    chords
        .into_iter()
        .filter_map(|s| gpui::Keystroke::parse(&s).ok().map(|s| s.unparse()))
        .collect()
}
