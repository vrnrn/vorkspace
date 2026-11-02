use crate::controls::secondary_button;
use crate::settings::SettingsView;
use gpui::prelude::*;
use gpui::{AnyElement, Context};
use vorkspace_clipboard::ModelId;

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
        let mut rows = rows;
        let selected = self.prefs.dictation_model;
        let selector = self.select(
            "dictation-model-selection",
            "Dictation model",
            selected.key(),
            ModelId::ALL
                .into_iter()
                .map(|id| (id.key().into(), id.descriptor().label.into(), None))
                .collect(),
            Some(232.),
            cx,
            |this, value, cx| {
                if let Some(model) = ModelId::from_key(&value) {
                    this.choose_dictation_model(model, cx);
                }
            },
        );
        rows.push(self.row(Some("dictation-model-selection"),false,"Model",Some("0.6B · Q8 GGUF. Changing model stops dictation and leaves it off. Download if prompted, then enable when ready.".into()),vec![selector]));
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
        rows.push(self.row(Some("dictation-enabled"), true, "Local dictation", Some("Press ⌘, to start, then ⌘, to transcribe and insert into the selected text field. Transcribed on your Mac. Works after quitting Vorkspace. Audio is never saved. Vox uses ⌘.".into()), vec![self.toggle("dictation-enabled", "Enable local dictation", self.prefs.dictation_enabled, false, cx, |this,on,cx|this.choose_dictation(on,cx))]));
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
        self.group(
            Some("clipboard"),
            false,
            "Clipboard & Dictation",
            None,
            None,
            rows,
        )
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
}
