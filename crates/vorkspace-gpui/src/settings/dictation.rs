use super::{SettingsView, widgets::secondary_button};
use gpui::{AnyElement, Context, div, prelude::*, px};
use vorkspace_helper::{DictationPhase, word_rules};
use vorkspace_ui::theme::content;

impl SettingsView {
    pub(super) fn dictation_group(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let shortcut = super::pages::command_shortcut(&self.prefs, "App: Dictation")
            .unwrap_or_else(|| "the shortcut in Keybindings".into());
        let mut rows = vec![self.row(
            Some("dictation-enabled"), true, "Enable dictation",
            Some(format!("Tap {shortcut} to toggle, or hold to dictate until release. Escape cancels. Works after Vorkspace quits.").into()),
            vec![self.toggle("dictation-enabled", "Enable dictation", self.prefs.dictation_enabled,
                false, cx, |this, on, cx| this.update(cx, |p| p.dictation_enabled = on))],
        )];
        let configured = match self.prefs.keybindings.get("App: Dictation") {
            Some(value) => value.clone().unwrap_or_default(),
            None => vorkspace_helper::DictationConfig::default().shortcut,
        };
        let mut keys = vec![
            ("fn".into(), "Fn / Globe".into(), None),
            ("right-alt".into(), "Right Option".into(), None),
            (String::new(), "Disabled".into(), None),
        ];
        if !["fn", "right-alt", ""].contains(&configured.as_str()) {
            keys.push((configured.clone(), shortcut.clone().into(), None));
        }
        rows.push(self.row(
            Some("dictation-key"), false, "Dictation key",
            Some("One key for tap to toggle and hold to talk. Fn combinations still work. Set macOS Keyboard’s Globe-key action to Do Nothing to avoid conflicting system actions.".into()),
            vec![self.select("dictation-key", "Dictation key", &configured, keys, Some(240.), cx,
                |this, key, cx| this.update(cx, |p| {
                    p.keybindings.insert("App: Dictation".into(), (!key.is_empty()).then_some(key));
                }))],
        ));
        if self.prefs.dictation_enabled {
            let status = crate::background_helper::get(cx)
                .map(|s| s.read(cx).dictation_status.clone())
                .unwrap_or_default();
            let action = match status.phase {
                DictationPhase::Microphone => Some("Allow microphone"),
                DictationPhase::Accessibility => Some("Open Accessibility Settings"),
                DictationPhase::Model => Some("Download language"),
                DictationPhase::Error | DictationPhase::Off => Some("Try again"),
                _ => None,
            };
            rows.push(
                self.row(
                    Some("dictation-status"),
                    false,
                    "Speech recognition",
                    Some(if status.message.is_empty() {
                        "Connecting to the background helper…".into()
                    } else {
                        status.message.into()
                    }),
                    action
                        .into_iter()
                        .map(|label| {
                            secondary_button("dictation-setup", label, false, false)
                                .child(label)
                                .on_click(|_, _, cx| crate::background_helper::dictation_setup(cx))
                                .into_any_element()
                        })
                        .collect(),
                ),
            );
        }
        let mut options = vec![
            ("system".into(), "System default".into(), None),
            ("builtin".into(), "Built-in microphone".into(), None),
        ];
        #[cfg(not(test))]
        options.extend(
            vorkspace_helper::input_devices()
                .into_iter()
                .map(|device| (format!("uid:{}", device.uid), device.name.into(), None)),
        );
        if !options
            .iter()
            .any(|(id, _, _)| id == &self.prefs.dictation_microphone)
        {
            options.push((
                self.prefs.dictation_microphone.clone(),
                "Selected microphone (unavailable)".into(),
                None,
            ));
        }
        rows.push(self.row(
            Some("dictation-microphone"),
            false,
            "Microphone",
            Some("Applies to the next dictation.".into()),
            vec![self.select(
                "dictation-microphone",
                "Dictation microphone",
                &self.prefs.dictation_microphone,
                options,
                Some(240.),
                cx,
                |this, value, cx| this.update(cx, |p| p.dictation_microphone = value),
            )],
        ));
        let changed =
            self.dictation_words_input.read(cx).content.as_ref() != self.prefs.dictation_words;
        let description = self.dictation_words_error.clone().unwrap_or_else(||
                "Preferred spellings, one per line. To correct a misheard term: work space → Vorkspace. Changes apply to the next dictation.".into());
        let editor = div()
            .text_color(content(1.))
            .w(px(320.))
            .max_w_full()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .rounded_md()
                    .border_1()
                    .border_color(content(0.15))
                    .bg(content(0.03))
                    .px_3()
                    .py_2()
                    .child(self.dictation_words_input.clone()),
            )
            .child(
                secondary_button("dictation-words-save", "Save custom words", false, !changed)
                    .debug_selector(|| "dictation-words-save".into())
                    .child("Save custom words")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let words = this.dictation_words_input.read(cx).content.to_string();
                        match word_rules(&words) {
                            Ok(_) => {
                                this.dictation_words_error = None;
                                this.update(cx, |p| p.dictation_words = words);
                            }
                            Err(error) => {
                                this.dictation_words_error = Some(error);
                                cx.notify();
                            }
                        }
                    })),
            );
        rows.push(self.row(
            Some("dictation-words"),
            false,
            "Custom words",
            Some(description.into()),
            vec![editor.into_any_element()],
        ));

        self.group(Some("dictation"), false, "Dictation", None, None, rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, Render, Window};
    use vorkspace_ui::preferences::Preferences;

    struct Fixture {
        view: gpui::Entity<SettingsView>,
        _observe: gpui::Subscription,
    }
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let scroll = self.view.read(cx).scroll.clone();
            div()
                .id("dictation-settings-fixture")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&scroll)
                .bg(vorkspace_ui::theme::shell_fill())
                .p_4()
                .child(self.view.update(cx, |v, cx| v.dictation_group(cx)))
        }
    }

    #[gpui::test]
    fn words_validate_before_save_and_external_preferences_preserve_a_draft(
        cx: &mut gpui::TestAppContext,
    ) {
        let view = cx.update(|cx| {
            cx.new(|cx| {
                SettingsView::new(
                    super::super::Section::General,
                    Preferences::default(),
                    Default::default(),
                    cx,
                )
            })
        });
        view.update(cx, |view, cx| {
            view.dictation_words_input
                .update(cx, |input, cx| input.set_text("my unsaved term".into(), cx));
            let updated = Preferences {
                dictation_words: "external term".into(),
                ..Default::default()
            };
            view.set_preferences(&updated, cx);
            assert_eq!(
                view.dictation_words_input.read(cx).content.as_ref(),
                "my unsaved term"
            );
            assert!(word_rules("term → One\nTERM → Other").is_err());
            assert!(
                super::super::model::search("custom words")
                    .iter()
                    .any(|result| result.setting == Some("dictation-words"))
            );
        });
    }

    #[test]
    #[ignore = "Requires Metal and VORKSPACE_DICTATION_CAPTURE_DIR; isolated Settings fixture"]
    fn dictation_settings_captures() {
        use std::sync::Arc;
        let output =
            std::path::PathBuf::from(std::env::var_os("VORKSPACE_DICTATION_CAPTURE_DIR").unwrap());
        assert!(output.is_absolute() && output.is_dir());
        for (appearance, light) in [("dark", false), ("light", true)] {
            for (viewport, width) in [("wide", 960.), ("narrow", 480.)] {
                let prefs = Preferences {
                    dictation_enabled: true,
                    dictation_words: "Vorkspace\nwork space → Vorkspace\nclawed → Claude".into(),
                    theme: appearance.into(),
                    ..Default::default()
                };
                vorkspace_ui::theme::configure(&prefs, light, false);
                let mut cx = gpui::HeadlessAppContext::with_platform(
                    Arc::new(gpui_apple::AppleTextSystem::new()),
                    Arc::new(crate::assets::Assets),
                    || {
                        Ok(Some(Box::new(
                            gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
                        )))
                    },
                );
                vorkspace_ui::clock::use_test_clock(&cx);
                let window = cx
                    .open_window(gpui::size(px(width), px(620.)), |_, cx| {
                        crate::background_helper::ensure(cx).update(cx, |service, _| {
                            service.dictation_status = vorkspace_helper::DictationStatus {
                                phase: DictationPhase::Ready,
                                message: "Ready. Speech stays on this Mac.".into(),
                                active: false,
                            };
                        });
                        let view = cx.new(|cx| {
                            SettingsView::new(
                                super::super::Section::General,
                                prefs,
                                Default::default(),
                                cx,
                            )
                        });
                        cx.new(|cx| Fixture {
                            _observe: cx.observe(&view, |_, _, cx| cx.notify()),
                            view,
                        })
                    })
                    .unwrap();
                cx.run_until_parked();
                vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(output.join(format!("settings-{appearance}-{viewport}.png")))
                    .unwrap();
            }
        }
    }
}
