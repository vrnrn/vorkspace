//! Explicit, unsaved consent until a model has passed integrity verification.
use super::*;
use crate::controls::{primary_button, secondary_button};
#[cfg(not(test))]
use vorkspace_clipboard::dictation_model;
use vorkspace_clipboard::{ModelId, ModelState, dictation_path};
#[derive(Clone, Debug)]
pub(super) struct Onboarding {
    pub request: u64,
    pub model: ModelId,
    destination: Result<std::path::PathBuf, String>,
    enable: bool,
    phase: ModelState,
}
impl Onboarding {
    pub(super) fn checking(&self) -> bool {
        self.phase == ModelState::Checking
    }
}
impl SettingsView {
    pub(super) fn choose_dictation_model(&mut self, model: ModelId, cx: &mut Context<Self>) {
        if self.prefs.dictation_model == model {
            return;
        }
        self.cancel_dictation_setup(cx);
        crate::clipboard::select_model(model, cx);
        self.update(cx, |p| {
            p.dictation_enabled = false;
            p.dictation_model = model;
        });
        self.begin_dictation_setup(false, cx);
    }
    pub(super) fn choose_dictation(&mut self, on: bool, cx: &mut Context<Self>) {
        if !on {
            self.cancel_dictation_setup(cx);
            self.update(cx, |p| p.dictation_enabled = false);
            crate::clipboard::dictation_cancel(cx);
            return;
        }
        self.begin_dictation_setup(true, cx);
    }
    pub(super) fn begin_dictation_setup(&mut self, enable: bool, cx: &mut Context<Self>) {
        if self.dictation_onboarding.is_some() {
            return;
        }
        let request = crate::clipboard::next_model_request();
        let model = self.prefs.dictation_model;
        let destination = dictation_path::model_path(model);
        self.dictation_onboarding = Some(Onboarding {
            request,
            model,
            destination: destination.clone(),
            enable,
            phase: ModelState::Checking,
        });
        cx.notify();
        #[cfg(not(test))]
        cx.spawn(async move |this, cx| {
            let ready = cx
                .background_executor()
                .spawn(async move { destination.map(|p| dictation_model::verified(model, &p)) })
                .await;
            let _ = this.update(cx, |this, cx| this.dictation_checked(request, ready, cx));
        })
        .detach();
        #[cfg(test)]
        let _ = destination;
    }
    fn dictation_checked(
        &mut self,
        request: u64,
        result: Result<bool, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(onboarding) = &mut self.dictation_onboarding else {
            return;
        };
        if onboarding.model != self.prefs.dictation_model
            || onboarding.request != request
            || onboarding.phase != ModelState::Checking
        {
            return;
        }
        onboarding.phase = match result {
            Ok(true) => ModelState::Ready,
            Ok(false) => ModelState::Missing,
            Err(e) => ModelState::Error(e),
        };
        if onboarding.phase == ModelState::Ready {
            self.finish_dictation_setup(cx);
        } else {
            cx.notify();
        }
    }
    pub(super) fn cancel_dictation_setup(&mut self, cx: &mut Context<Self>) {
        if let Some(onboarding) = self.dictation_onboarding.take() {
            crate::clipboard::cancel_model(onboarding.request, onboarding.model, cx);
        }
        cx.notify();
    }
    fn finish_dictation_setup(&mut self, cx: &mut Context<Self>) {
        let Some(onboarding) = self.dictation_onboarding.take() else {
            return;
        };
        if onboarding.model != self.prefs.dictation_model {
            return;
        }
        if onboarding.enable {
            self.update(cx, |p| p.dictation_enabled = true);
        } else {
            crate::clipboard::finish_model(onboarding.request, onboarding.model, cx);
        }
        cx.notify();
    }
    fn confirm_dictation_download(&mut self, cx: &mut Context<Self>) {
        let Some(onboarding) = &mut self.dictation_onboarding else {
            return;
        };
        if onboarding.destination.is_err()
            || !matches!(
                onboarding.phase,
                ModelState::Missing
                    | ModelState::Ready
                    | ModelState::Cancelled
                    | ModelState::Error(_)
            )
        {
            return;
        }
        // The only network admission point is this explicit Download action.
        let request = crate::clipboard::next_model_request();
        onboarding.request = request;
        match crate::clipboard::dictation_download(request, onboarding.model, cx) {
            Ok(()) => onboarding.phase = ModelState::Downloading(0),
            Err(e) => onboarding.phase = ModelState::Error(e),
        }
        cx.notify();
    }
    pub(super) fn dictation_notice(&mut self, cx: &mut Context<Self>) {
        let notice = crate::clipboard::get(cx).and_then(|s| s.read(cx).model_notice.clone());
        let Some((request, model, state)) = notice else {
            return;
        };
        let Some(onboarding) = &mut self.dictation_onboarding else {
            return;
        };
        if model != onboarding.model
            || model != self.prefs.dictation_model
            || onboarding.request != request
            || !matches!(
                onboarding.phase,
                ModelState::Checking | ModelState::Downloading(_) | ModelState::Cancelling
            )
        {
            return;
        }
        onboarding.phase = state;
        if onboarding.phase == ModelState::Ready {
            self.finish_dictation_setup(cx);
        } else {
            cx.notify();
        }
    }
    pub(super) fn dictation_overlay(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let onboarding = self.dictation_onboarding.as_ref()?;
        // Consent is a brief decision. Checking and acquisition stay in the
        // existing Settings status row, where Cancel remains available.
        if matches!(
            onboarding.phase,
            ModelState::Checking | ModelState::Downloading(_) | ModelState::Cancelling
        ) {
            return None;
        }
        let status = match &onboarding.phase {
            ModelState::Checking => {
                "Checking the model or connecting to the background helper…".into()
            }
            ModelState::Missing => "Dictation stays off until the download is verified.".into(),
            ModelState::Ready => {
                "The existing model is verified. Download only if you want to replace it.".into()
            }
            ModelState::Downloading(bytes) if *bytes == onboarding.model.descriptor().bytes => {
                "Verifying download…".into()
            }
            ModelState::Downloading(bytes) => {
                format!(
                    "Downloading… {}%",
                    bytes * 100 / onboarding.model.descriptor().bytes
                )
            }
            ModelState::Cancelling => "Cancelling download…".into(),
            ModelState::Cancelled => "Download cancelled. Dictation is still off.".into(),
            ModelState::Error(e) => e.clone(),
        };
        let destination = onboarding
            .destination
            .as_ref()
            .map(|p| p.parent().unwrap().display().to_string())
            .unwrap_or_else(|e| e.clone());
        let busy = matches!(
            onboarding.phase,
            ModelState::Checking | ModelState::Downloading(_) | ModelState::Cancelling
        );
        let downloadable = !busy && onboarding.destination.is_ok();
        let width = 480f32.min(window.viewport_size().width.as_f32() - 32.);
        // Match the existing Settings management dialog composition.
        let mut fill = shell_fill();
        fill.a = 1.;
        Some(div().absolute().inset_0().occlude().bg(black(0.4))
            .flex().items_center().justify_center()
            .on_mouse_down(MouseButton::Left,|_,_,cx|cx.stop_propagation())
            .child(div().id("dictation-download-prompt")
                .debug_selector(||"dictation-download-prompt".into())
                .role(Role::Dialog).aria_label("Download local dictation model")
                .w(px(width)).max_w_full().rounded_xl().bg(fill)
                .border_1().border_color(content(0.1)).text_color(content(1.))
                .p_4().flex().flex_col().gap_3()
                .child(div().text_size(px(16.)).font_weight(gpui::FontWeight::SEMIBOLD).child("Download dictation model?"))
                .child(div().text_size(px(13.)).child(format!("{} · about {} MB",onboarding.model.descriptor().label, (onboarding.model.descriptor().bytes + 500_000) / 1_000_000)))
                .child(div().text_size(px(12.)).text_color(content(0.85)).child("Downloaded once for all Vorkspace installs on this Mac. Speech is transcribed locally."))
                .child(div().text_size(px(12.)).child(format!("Save to: {destination}")))
                .child(div().id("dictation-setup-status").text_size(px(12.)).text_color(content(0.85)).child(status))
                .child(div().flex().justify_end().gap_2()
                    .child(secondary_button("dictation-consent-cancel","Cancel",false,false).debug_selector(||"dictation-consent-cancel".into()).child("Cancel").on_click(cx.listener(|this,_,_,cx|this.cancel_dictation_setup(cx))))
                    .child(primary_button("dictation-consent-download","Download",!downloadable).debug_selector(||"dictation-consent-download".into()).child("Download").on_click(cx.listener(|this,_,_,cx|this.confirm_dictation_download(cx))))))
            .into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn view(cx: &mut gpui::TestAppContext) -> Entity<SettingsView> {
        cx.new(|cx| {
            SettingsView::new(
                Section::General,
                Preferences::default(),
                SettingsContext::default(),
                cx,
            )
        })
    }
    #[gpui::test]
    fn enable_checks_then_prompts_without_persisting_or_downloading_and_cancel_stays_off(
        cx: &mut gpui::TestAppContext,
    ) {
        let view = view(cx);
        view.update(cx, |v, cx| {
            v.choose_dictation(true, cx);
            let id = v.dictation_onboarding.as_ref().unwrap().request;
            assert!(!v.prefs.dictation_enabled);
            assert!(
                crate::clipboard::get(cx)
                    .unwrap()
                    .read(cx)
                    .model_notice
                    .is_none()
            );
            v.dictation_checked(id, Ok(false), cx);
            assert_eq!(
                v.dictation_onboarding.as_ref().unwrap().phase,
                ModelState::Missing
            );
            assert!(!v.prefs.dictation_enabled);
            v.cancel_dictation_setup(cx);
            v.dictation_checked(id, Ok(true), cx);
            assert!(!v.prefs.dictation_enabled);
            assert!(v.dictation_onboarding.is_none());
        });
    }
    #[gpui::test]
    fn verified_existing_model_enables_durably_without_any_acquisition(
        cx: &mut gpui::TestAppContext,
    ) {
        let view = view(cx);
        view.update(cx, |v, cx| {
            v.choose_dictation(true, cx);
            let id = v.dictation_onboarding.as_ref().unwrap().request;
            v.dictation_checked(id, Ok(true), cx);
            assert!(v.prefs.dictation_enabled);
            assert!(v.dictation_onboarding.is_none());
            let saved = serde_json::to_string(&v.prefs).unwrap();
            assert!(
                serde_json::from_str::<Preferences>(&saved)
                    .unwrap()
                    .dictation_enabled
            );
            assert!(
                crate::clipboard::get(cx)
                    .unwrap()
                    .read(cx)
                    .model_notice
                    .is_none()
            );
        });
    }
    #[gpui::test]
    fn explicit_download_is_queued_while_disconnected_and_late_cancelled_reply_cannot_enable(
        cx: &mut gpui::TestAppContext,
    ) {
        let view = view(cx);
        view.update(cx, |v, cx| {
            v.choose_dictation(true, cx);
            let first = v.dictation_onboarding.as_ref().unwrap().request;
            v.dictation_checked(first, Ok(false), cx);
            v.confirm_dictation_download(cx);
            let cancelled = v.dictation_onboarding.as_ref().unwrap().request;
            assert_ne!(first, cancelled);
            assert!(!v.prefs.dictation_enabled);
            let service = crate::clipboard::get(cx).unwrap();
            assert_eq!(
                service.read(cx).model_notice,
                Some((cancelled, ModelId::default(), ModelState::Checking))
            );
            v.cancel_dictation_setup(cx);
            v.choose_dictation(true, cx);
            let next = v.dictation_onboarding.as_ref().unwrap().request;
            assert_ne!(cancelled, next);
            service.update(cx, |s, cx| {
                s.test_model_reply(cancelled, ModelState::Ready, cx)
            });
            v.dictation_notice(cx);
            assert!(!v.prefs.dictation_enabled);
            assert_eq!(v.dictation_onboarding.as_ref().unwrap().request, next);
        });
    }
    #[gpui::test]
    fn only_matching_verified_download_completion_enables(cx: &mut gpui::TestAppContext) {
        let view = view(cx);
        view.update(cx, |v, cx| {
            v.choose_dictation(true, cx);
            let id = v.dictation_onboarding.as_ref().unwrap().request;
            v.dictation_checked(id, Ok(false), cx);
            v.confirm_dictation_download(cx);
            let id = v.dictation_onboarding.as_ref().unwrap().request;
            let service = crate::clipboard::get(cx).unwrap();
            service.update(cx, |s, cx| {
                s.test_model_reply(id, ModelState::Downloading(1024), cx)
            });
            v.dictation_notice(cx);
            assert!(!v.prefs.dictation_enabled);
            service.update(cx, |s, cx| s.test_model_reply(id, ModelState::Ready, cx));
            v.dictation_notice(cx);
            assert!(v.prefs.dictation_enabled);
            assert!(v.dictation_onboarding.is_none());
        });
    }
}

#[cfg(test)]
mod preview_tests {
    use super::*;
    #[test]
    #[ignore = "Requires Metal and VORKSPACE_DICTATION_SETUP_CAPTURE_DIR; display-only consent preview"]
    fn dictation_onboarding_captures() {
        use std::sync::Arc;
        let output = std::path::PathBuf::from(
            std::env::var_os("VORKSPACE_DICTATION_SETUP_CAPTURE_DIR").unwrap(),
        );
        assert!(output.is_absolute() && output.is_dir());
        let states=[("prompt",ModelState::Missing),("connecting",ModelState::Checking),("downloading",ModelState::Downloading(365_000_000)),("error",ModelState::Error("Another Vorkspace install is downloading this model. Wait for it to finish, then retry.".into()))];
        let selected = std::env::var("VORKSPACE_DICTATION_SETUP_STATE").ok();
        if selected.is_none() {
            for (name, _) in &states {
                let status = std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "settings::dictation::preview_tests::dictation_onboarding_captures",
                        "--ignored",
                    ])
                    .env("VORKSPACE_DICTATION_SETUP_STATE", name)
                    .status()
                    .unwrap();
                assert!(status.success());
            }
            return;
        }
        for (name, phase) in states {
            if selected.as_deref() != Some(name) {
                continue;
            }
            // Independent full scenes: the offscreen renderer may omit reused
            // glyph runs on incremental readback. This fixture claims state layout,
            // not a live transition or physical window interaction.
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
                .open_window(gpui::size(px(1000.), px(720.)), |_, cx| {
                    cx.new(|cx| {
                        let mut view = SettingsView::new(
                            Section::General,
                            Preferences::default(),
                            SettingsContext::default(),
                            cx,
                        );
                        view.discovery_task = None;
                        view.discovery_loading = false;
                        view.dictation_onboarding = Some(Onboarding {
                            request: 1,
                            model: ModelId::default(),
                            destination: Ok(std::path::PathBuf::from(
                                "/Users/example/.vorkspace/dictation",
                            )
                            .join(ModelId::default().descriptor().filename)),
                            enable: true,
                            phase,
                        });
                        view
                    })
                })
                .unwrap();
            cx.run_until_parked();
            vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(output.join(format!("dictation-setup-{name}.png")))
                .unwrap();
        }
    }
}
