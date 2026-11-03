use crate::controls::secondary_button;
use crate::settings::SettingsView;
use gpui::prelude::*;
use gpui::{AnyElement, Context, div, px};
use vorkspace_clipboard::ModelId;
use vorkspace_ui::theme::content;

impl SettingsView {
    pub(super) fn clipboard_group(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let controls = vec![self.toggle(
            "clipboard-history",
            "Clipboard",
            self.prefs.clipboard_history,
            false,
            cx,
            |this, on, cx| this.update(cx, |p| p.clipboard_history = on),
        )];
        let description = "Keep up to 50 text and image copies in memory, even after you quit, until you log out or use Quit in the clipboard menu. History is never saved to disk. Turning this off clears it. Very large copies are skipped.";
        let description = if let Some(service) = crate::clipboard::get(cx)
            && let Some(error) = &service.read(cx).error
        {
            format!("{description}\n{error}")
        } else {
            description.to_owned()
        };
        let row = self.row(
            Some("clipboard-history"),
            true,
            "Keep recent copies",
            Some(description.into()),
            controls,
        );
        let rows = vec![row];
        let rows = {
            let mut rows = rows;
            rows.push(self.row(
                Some("clipboard-clear"),
                false,
                "Saved copies",
                Some("Clearing saved copies leaves the current system clipboard unchanged.".into()),
                vec![secondary_button("clipboard-clear-button", "Clear history", false, false)
                    .child("Clear history")
                    .on_click(|_, _, cx| crate::clipboard::clear(cx))
                    .into_any_element()],
            ));
            rows
        };
        self.group(Some("clipboard"), false, "Clipboard", None, None, rows)
    }

    pub(super) fn dictation_group(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let mut rows = Vec::new();
        let selected = self.prefs.dictation_model;
        let flash_supported = vorkspace_clipboard::dictation_worker::realtime_supported();
        let selector = self.select(
            "dictation-model-selection",
            "Dictation model",
            selected.key(),
            ModelId::ALL
                .into_iter()
                .filter(|id| {
                    *id != ModelId::ParakeetRealtimeEou || flash_supported || *id == selected
                })
                .map(|id| {
                    let label = match id {
                        ModelId::ParakeetTdtV3 => "Parakeet-TDT v3 (Experimental)",
                        ModelId::ParakeetRealtimeEou if !flash_supported => {
                            "Parakeet Flash (Experimental, unavailable)"
                        }
                        ModelId::ParakeetRealtimeEou => "Parakeet Flash (Experimental)",
                        _ => id.descriptor().label,
                    };
                    (id.key().into(), label.into(), None)
                })
                .collect(),
            Some(232.),
            cx,
            |this, value, cx| {
                if let Some(model) = ModelId::from_key(&value) {
                    this.choose_dictation_model(model, cx);
                }
            },
        );
        let model_description = match selected {
            ModelId::UnifiedEnglish => "English with punctuation and capitalization.",
            ModelId::ParakeetTdtV3 => "25 European languages with punctuation and capitalization.",
            ModelId::ParakeetRealtimeEou if !flash_supported => {
                "Requires Apple silicon and macOS 14 or newer. Choose another model on this Mac."
            }
            ModelId::ParakeetRealtimeEou => "Fast English without punctuation or capitalization.",
        };
        rows.push(
            self.row(
                Some("dictation-model-selection"),
                true,
                "Model",
                Some(
                    format!(
                        "{model_description} Changing model stops dictation and leaves it off."
                    )
                    .into(),
                ),
                vec![selector],
            ),
        );
        let devices = crate::clipboard::get(cx)
            .map(|s| s.read(cx).microphones.clone())
            .unwrap_or_default();
        let builtin = devices.iter().find(|d| d.builtin);
        let mut choices = vec![
            (
                "builtin".into(),
                builtin
                    .map(|d| format!("Built-in · {}", d.name))
                    .unwrap_or_else(|| "Built-in microphone (unavailable)".into()),
                None,
            ),
            (
                "system".into(),
                if devices.is_empty() {
                    "System default (no input available)"
                } else {
                    "System default"
                }
                .into(),
                None,
            ),
        ];
        choices.extend(
            devices
                .iter()
                .map(|d| (format!("uid:{}", d.uid), d.name.clone(), None)),
        );
        let microphone = self.prefs.dictation_microphone.clone();
        if !choices.iter().any(|(id, _, _)| id == &microphone) {
            choices.push((
                microphone.clone(),
                "Saved microphone (unavailable)".into(),
                None,
            ));
        }
        let unavailable = (microphone == "system" && devices.is_empty())
            || (microphone == "builtin" && builtin.is_none())
            || (microphone != "builtin"
                && microphone != "system"
                && !devices
                    .iter()
                    .any(|d| microphone == format!("uid:{}", d.uid)));
        let selector = self.select(
            "dictation-microphone",
            "Dictation microphone",
            &microphone,
            choices
                .into_iter()
                .map(|(id, label, icon)| (id, label.into(), icon))
                .collect(),
            Some(232.),
            cx,
            |this, value, cx| this.update(cx, |p| p.dictation_microphone = value.clone()),
        );
        rows.push(self.row(Some("dictation-microphone"), false, "Microphone", Some(if unavailable {
            "Selected input unavailable. Connect it or choose another microphone. Built-in uses the Mac’s internal input; Macs without one must choose System default or a device. Applies to the next recording."
        } else {
            "Uses the Mac’s built-in microphone by default. Applies to the next recording. Devices refresh automatically; a disconnected input stops recording without switching microphones."
        }.into()), vec![selector]));
        rows.push(self.row(Some("dictation-enabled"), false, "Local dictation", Some(match self.prefs.dictation_model {
            vorkspace_clipboard::ModelId::UnifiedEnglish => "Live on your Mac: stable words appear in the focused field as you speak. Press ⌘, to start or finish. Editing or changing focus stops further insertion. Audio is never saved. Works after Quit; Vox uses ⌘.",
            vorkspace_clipboard::ModelId::ParakeetTdtV3 => "Live on your Mac: stable words appear in the focused field as you speak. Press ⌘, to start or finish. Editing or changing focus stops further insertion. Audio is never saved. Works after Quit; Vox uses ⌘.",
            vorkspace_clipboard::ModelId::ParakeetRealtimeEou => "Live on your Mac: stable words appear in the focused field as you speak. Press ⌘, to start or finish. Editing or changing focus stops further insertion. Audio is never saved. Works after Quit; Vox uses ⌘.",
        }.into()), vec![self.toggle("dictation-enabled", "Enable local dictation", self.prefs.dictation_enabled, false, cx, |this,on,cx|this.choose_dictation(on,cx))]));
        let draft = self.dictation_words_input.read(cx).content.to_string();
        let changed = draft != self.prefs.dictation_words;
        let explanation = self.dictation_words_error.clone().unwrap_or_else(||
            "One term per line corrects capitalization when recognized. For misheard words, use heard phrase => preferred spelling. TDT also scores speech using these terms; Unified and Flash correct text only. Saved for the next recording.".into());
        let words_editor = div()
            .w(px(280.))
            .max_w_full()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .w_full()
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
                        let text = this.dictation_words_input.read(cx).content.to_string();
                        match vorkspace_core::dictation_words::parse(&text) {
                            Ok(_) => {
                                this.dictation_words_error = None;
                                this.update(cx, |p| p.dictation_words = text);
                            }
                            Err(error) => {
                                this.dictation_words_error = Some(error);
                                cx.notify();
                            }
                        }
                    })),
            )
            .into_any_element();
        rows.push(self.row(
            Some("dictation-words"),
            false,
            "Custom words",
            Some(explanation.into()),
            vec![words_editor],
        ));
        let (status, ready) = crate::clipboard::get(cx)
            .map(|s| {
                let s = s.read(cx);
                (s.dictation_status.clone(), s.dictation_ready)
            })
            .unwrap_or_default();
        let enabled = self.prefs.dictation_enabled;
        let setting_up = self.dictation_onboarding.is_some();
        rows.push(self.row(
            Some("dictation-model"),
            false,
            "Selected model",
            Some(if setting_up && self.dictation_onboarding.as_ref().is_some_and(|o|o.checking()) {
                "Checking model or connecting to background helper… Cancel to leave dictation off.".into()
            } else if status.is_empty() {
                "Enable dictation to download the model and set up permissions.".into()
            } else {
                status.into()
            }),
            vec![
                    secondary_button("dictation-download", "Download model", false, !enabled)
                        .child(if ready {
                            "Check model"
                        } else {
                            "Download model"
                        })
                        .on_click(
                            cx.listener(|this, _, _, cx| this.begin_dictation_setup(false, cx)),
                        )
                        .into_any_element(),
                    secondary_button(
                        "dictation-permissions",
                        "Set up permissions",
                        false,
                        !enabled,
                    )
                    .child("Set up permissions")
                    .on_click(|_, _, cx| crate::clipboard::dictation_permissions(cx))
                    .into_any_element(),
                    secondary_button("dictation-cancel", "Cancel dictation", false, !enabled && !setting_up)
                        .child("Cancel")
                        .on_click(cx.listener(|this, _, _, cx| {this.cancel_dictation_setup(cx);crate::clipboard::dictation_cancel(cx);}))
                        .into_any_element(),
                ],
        ));
        rows.push(self.row(None, false, "Permissions", Some("Allow Microphone and Accessibility for vorkspace Clipboard, the bundled background helper. If access was denied, enable it in System Settings → Privacy & Security. Keep the intended field focused until insertion completes.".into()), vec![]));
        self.group(Some("dictation"), false, "Dictation", None, None, rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        AppContext, Bounds, Render, TestAppContext, VisualTestContext, Window, WindowBounds,
        WindowOptions, div, point, px, size,
    };
    struct GroupFixture {
        view: gpui::Entity<SettingsView>,
        _observe: gpui::Subscription,
    }
    impl Render for GroupFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .p_4()
                .w_full()
                .child(self.view.update(cx, |view, cx| view.clipboard_group(cx)))
                .child(self.view.update(cx, |view, cx| view.dictation_group(cx)))
        }
    }
    #[test]
    #[ignore = "Requires Metal and VORKSPACE_DICTATION_SETTINGS_CAPTURE_DIR; display-only settings fixture"]
    fn dictation_settings_captures() {
        use std::sync::Arc;
        let output = std::path::PathBuf::from(
            std::env::var_os("VORKSPACE_DICTATION_SETTINGS_CAPTURE_DIR").unwrap(),
        );
        assert!(output.is_absolute() && output.is_dir());
        for (name, width) in [("wide", 1000.), ("narrow", 620.)] {
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
                .open_window(size(px(width), px(1100.)), |_, cx| {
                    let view = cx.new(|cx| {
                        SettingsView::new(
                            crate::settings::Section::General,
                            Default::default(),
                            Default::default(),
                            cx,
                        )
                    });
                    cx.new(|cx| GroupFixture {
                        _observe: cx.observe(&view, |_, _, cx| cx.notify()),
                        view,
                    })
                })
                .unwrap();
            cx.run_until_parked();
            vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(output.join(format!("settings-{name}.png")))
                .unwrap();
        }
    }
    #[gpui::test]
    fn microphone_dropdown_routes_uid_and_retains_unavailable_choice(cx: &mut TestAppContext) {
        let view = cx.update(|cx| {
            crate::clipboard::ensure(cx).update(cx, |service, _| {
                service.microphones = vec![vorkspace_clipboard::InputDevice {
                    uid: "fixture-usb".into(),
                    name: "Fixture USB mic".into(),
                    builtin: false,
                }];
            });
            cx.new(|cx| {
                SettingsView::new(
                    crate::settings::Section::General,
                    Default::default(),
                    Default::default(),
                    cx,
                )
            })
        });
        let window = cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(1000.), px(1000.)),
                    ))),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|cx| GroupFixture {
                        _observe: cx.observe(&view, |_, _, cx| cx.notify()),
                        view: view.clone(),
                    })
                },
            )
            .unwrap()
        });
        cx.refresh().unwrap();
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        assert_eq!(
            view.read_with(visual, |view, _| view.prefs.dictation_microphone.clone()),
            "builtin"
        );
        let trigger = visual
            .debug_bounds("settings-select-dictation-microphone")
            .unwrap();
        visual.simulate_click(trigger.center(), Default::default());
        visual.run_until_parked();
        assert_eq!(
            view.read_with(visual, |view, _| view.select_open),
            Some("dictation-microphone")
        );
        visual.simulate_keystrokes("down down enter");
        visual.run_until_parked();
        assert_eq!(
            view.read_with(visual, |view, _| view.prefs.dictation_microphone.clone()),
            "uid:fixture-usb"
        );
        visual.update(|_, cx| {
            crate::clipboard::ensure(cx).update(cx, |service, cx| {
                service.microphones.clear();
                cx.notify();
            });
        });
        visual.run_until_parked();
        assert_eq!(
            view.read_with(visual, |view, _| view.prefs.dictation_microphone.clone()),
            "uid:fixture-usb"
        );
        // The saved unavailable choice is still selectable; a new choice is explicit.
        let trigger = visual
            .debug_bounds("settings-select-dictation-microphone")
            .unwrap();
        visual.simulate_click(trigger.center(), Default::default());
        visual.run_until_parked();
        visual.simulate_keystrokes("home down enter");
        visual.run_until_parked();
        assert_eq!(
            view.read_with(visual, |view, _| view.prefs.dictation_microphone.clone()),
            "system"
        );
    }

    #[gpui::test]
    fn custom_word_editor_validates_saves_and_is_searchable(cx: &mut TestAppContext) {
        let view = cx.update(|cx| {
            cx.new(|cx| {
                SettingsView::new(
                    crate::settings::Section::General,
                    Default::default(),
                    Default::default(),
                    cx,
                )
            })
        });
        let window = cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(1000.), px(1200.)),
                    ))),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|cx| GroupFixture {
                        _observe: cx.observe(&view, |_, _, cx| cx.notify()),
                        view: view.clone(),
                    })
                },
            )
            .unwrap()
        });
        cx.refresh().unwrap();
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds("settings-row-clipboard-history")
                .is_some()
        );
        assert!(
            visual
                .debug_bounds("settings-row-dictation-words")
                .is_some()
        );
        assert!(
            crate::settings::model::search("custom words")
                .iter()
                .any(|r| r.setting == Some("dictation-words"))
        );
        visual.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.dictation_words_input
                    .update(cx, |input, cx| input.set_text("invalid =>".into(), cx))
            })
        });
        visual.run_until_parked();
        let save = visual.debug_bounds("dictation-words-save").unwrap();
        visual.simulate_click(save.center(), Default::default());
        visual.run_until_parked();
        assert!(view.read_with(visual, |view, _| view.dictation_words_error.is_some()));
        assert_eq!(
            view.read_with(visual, |view, _| view.prefs.dictation_words.clone()),
            ""
        );
        visual.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.dictation_words_input.update(cx, |input, cx| {
                    input.set_text("Vorkspace\nwork space => Vorkspace".into(), cx)
                })
            })
        });
        visual.run_until_parked();
        let save = visual.debug_bounds("dictation-words-save").unwrap();
        visual.simulate_click(save.center(), Default::default());
        visual.run_until_parked();
        assert_eq!(
            view.read_with(visual, |view, _| view.prefs.dictation_words.clone()),
            "Vorkspace\nwork space => Vorkspace"
        );
        assert!(!view.read_with(visual, |view, _| view.prefs.dictation_enabled));
    }
}
