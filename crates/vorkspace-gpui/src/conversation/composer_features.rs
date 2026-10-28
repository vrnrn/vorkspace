use super::*;
use std::path::PathBuf;
pub(crate) struct StartOrchestration(pub vorkspace_core::orchestration::Plan);
pub(crate) struct StopOrchestration;

impl Conversation {
    pub(crate) fn request_folder(&mut self, cx: &mut Context<Self>) {
        if self.settings.bot.is_none() {
            self.completion
                .update(cx, |completion, cx| completion.dismiss(cx));
            cx.emit(ChooseFolder(self.composer_anchor.clone()));
            cx.notify();
        }
    }
    pub(crate) fn folder_chosen(&mut self, cx: &mut Context<Self>) {
        self.folder_selected = true;
        let text = self.input.read(cx).content.clone();
        if text.trim().eq_ignore_ascii_case("/add-to-folder") {
            self.input
                .update(cx, |input, cx| input.set_text(format!("{text} "), cx));
        }
    }
    pub(crate) fn set_app_environment(
        &mut self,
        environment: std::collections::HashMap<String, String>,
    ) {
        self.app_environment = environment;
    }
    pub(crate) fn enable_mode(&mut self, id: &str, cx: &mut Context<Self>) {
        let enabled = match id {
            "draft" => self.follow_ups.draft_selected,
            "plan" => self.settings.plan,
            "operator" => self.settings.operator,
            "orchestrator" => self.settings.orchestrator,
            _ => true,
        };
        if !enabled {
            self.toggle_mode(id, cx);
        }
    }
    pub(crate) fn clear_mode(&mut self, id: &str, cx: &mut Context<Self>) {
        let enabled = match id {
            "draft" => &mut self.follow_ups.draft_selected,
            "plan" => &mut self.settings.plan,
            "operator" => &mut self.settings.operator,
            "orchestrator" => &mut self.settings.orchestrator,
            _ => return,
        };
        if *enabled {
            // The input has already been edited. Clearing its old mode must
            // preserve the replacement text and the user's caret position.
            *enabled = false;
            self.save_settings(cx);
            cx.notify();
        }
    }
    pub(super) fn clear_submitted_mode(&mut self, cx: &mut Context<Self>) {
        // The accepted turn or queue item already owns its mode. The empty
        // composer starts a fresh draft, matching Composer.tsx's submit reset.
        self.settings.plan = false;
        self.settings.operator = false;
        self.settings.orchestrator = false;
        self.input
            .update(cx, |input, cx| input.selected_command(None, cx));
        self.save_settings(cx);
        cx.notify();
    }
    pub(crate) fn toggle_mode(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.is_remote() && matches!(id, "operator" | "orchestrator") {
            self.status = "This mode is not available on remote projects.".into();
            cx.notify();
            return;
        }
        if id != "draft" {
            self.follow_ups.draft_selected = false;
        }
        match id {
            "draft" => {
                self.follow_ups.draft_selected = !self.follow_ups.draft_selected;
                self.settings.plan = false;
                self.settings.operator = false;
                self.settings.orchestrator = false;
            }
            "plan" => {
                self.settings.plan = !self.settings.plan;
                self.settings.orchestrator = false;
                self.settings.operator = false;
            }
            "operator" => {
                self.settings.operator = !self.settings.operator;
                self.settings.plan = false;
                self.settings.orchestrator = false;
            }
            "orchestrator" => {
                self.settings.orchestrator = !self.settings.orchestrator;
                self.settings.plan = false;
                self.settings.operator = false;
            }
            _ => return,
        }
        let selected = if self.follow_ups.draft_selected {
            Some("draft")
        } else if self.settings.orchestrator {
            Some("orchestrator")
        } else if self.settings.plan {
            Some("plan")
        } else if self.settings.operator {
            Some("operator")
        } else {
            None
        };
        self.input.update(cx, |input, cx| {
            let leading = input::Command::leading(&input.content);
            if leading.map(input::Command::name) != selected {
                let body = leading.map_or(input.content.as_ref(), |mode| {
                    input.content[mode.name().len() + 1..].trim_start()
                });
                let next =
                    selected.map_or_else(|| body.to_owned(), |name| format!("/{name} {body}"));
                input.set_text(next, cx);
            }
        });
        self.save_settings(cx);
        cx.notify();
    }
    pub(super) fn consume_mode_command(
        &mut self,
        mut text: String,
        cx: &mut Context<Self>,
    ) -> String {
        for mode in ["plan", "operator", "orchestrator"] {
            if let Some(rest) = vorkspace_core::composer::consume_command(&text, mode) {
                let rest = rest.to_owned();
                match mode {
                    "plan" => {
                        self.settings.plan = true;
                        self.settings.orchestrator = false;
                        self.settings.operator = false;
                    }
                    "operator" => {
                        self.settings.operator = true;
                        self.settings.plan = false;
                        self.settings.orchestrator = false;
                    }
                    _ => {
                        self.settings.orchestrator = true;
                        self.settings.plan = false;
                        self.settings.operator = false;
                    }
                }
                self.save_settings(cx);
                text = if rest.is_empty() && mode == "operator" {
                    "Explain what you can do in vorkspace with the app CLI.".into()
                } else {
                    rest
                };
                break;
            }
        }
        text
    }
    pub(super) fn choose_attachments(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach files".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                let _ = this.update(cx, |this, cx| this.add_attachments(paths, cx));
            }
        })
        .detach();
    }
    pub(super) fn paste_attachments(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(clipboard) = cx.read_from_clipboard() else {
            return false;
        };
        let paths: Vec<_> = clipboard
            .entries()
            .iter()
            .filter_map(|entry| match entry {
                gpui::ClipboardEntry::ExternalPaths(paths) => Some(paths.paths().to_vec()),
                _ => None,
            })
            .flatten()
            .collect();
        if !paths.is_empty() {
            self.add_attachments(paths, cx);
            return true;
        }
        let images: Vec<_> = clipboard
            .entries()
            .iter()
            .filter_map(|entry| match entry {
                gpui::ClipboardEntry::Image(image) => Some(image.clone()),
                _ => None,
            })
            .collect();
        if images.is_empty() {
            return false;
        }
        if self.attaching {
            return true;
        }
        self.attaching = true;
        let store = self.store.clone();
        let mut all = self.attachments.clone();
        let existing = all.len();
        let work = cx.background_executor().spawn(async move {
            if all.len() + images.len() > vorkspace_core::composer::MAX_ATTACHMENTS
                || images.iter().map(|i| i.bytes().len() as u64).sum::<u64>()
                    > vorkspace_core::composer::MAX_EMBED_BYTES
            {
                return Err("Attach up to 20 files totalling 20 MB.".to_owned());
            }
            for image in images {
                let path = locked(&store)?.cache_attachment(
                    &format!("pasted.{}", image.format().extension()),
                    image.bytes(),
                )?;
                all.push(path);
            }
            vorkspace_core::composer::validate_attachments(&all)?;
            Ok(all.split_off(existing))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.finish_attachment_read(result, cx);
            });
        })
        .detach();
        cx.notify();
        true
    }
    pub(super) fn add_attachments(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        for path in paths {
            if !self.attachments.contains(&path) && !self.pending_attachments.contains(&path) {
                self.pending_attachments.push(path);
            }
        }
        // Rapid successive drops must wait for the active read, rather than
        // silently losing the later files (including during an image paste).
        if self.attaching || self.pending_attachments.is_empty() {
            return;
        }
        let incoming = std::mem::take(&mut self.pending_attachments);
        let mut all = self.attachments.clone();
        all.extend(incoming.iter().cloned());
        self.attaching = true;
        let work = cx.background_executor().spawn(async move {
            vorkspace_core::composer::validate_attachments(&all).map(|_| incoming)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.finish_attachment_read(result, cx);
            });
        })
        .detach();
        cx.notify();
    }
    fn finish_attachment_read(
        &mut self,
        result: Result<Vec<PathBuf>, String>,
        cx: &mut Context<Self>,
    ) {
        self.attaching = false;
        match result {
            Ok(paths) => {
                // Keep removals made while the background validation ran.
                for path in paths {
                    if !self.attachments.contains(&path) {
                        self.attachments.push(path);
                    }
                }
                self.attachment_error = None;
            }
            Err(error) => self.attachment_error = Some(error),
        }
        let pending = std::mem::take(&mut self.pending_attachments);
        self.add_attachments(pending, cx);
        self.persist_composer(cx);
        cx.notify();
    }
    pub(crate) fn attachment_draft_generation(&self) -> u64 {
        self.composer_draft.generation()
    }

    pub(crate) fn annotation_draft_generation(&mut self, cx: &mut Context<Self>) -> u64 {
        // Capture input and attachment changes even if their notification has not run yet.
        self.persist_composer(cx);
        self.attachment_draft_generation()
    }

    /// Publish a newly encoded image only while this exact draft still owns its source.
    pub(crate) fn apply_annotated_attachment(
        &mut self,
        source: &std::path::Path,
        generation: u64,
        bytes: &[u8],
        cx: &mut Context<Self>,
    ) -> Result<PathBuf, String> {
        self.persist_composer(cx);
        if !self.composer_draft.ready_for_replacement()
            || self.attaching
            || self.composer_draft.generation() != generation
        {
            return Err(
                "The draft changed or is unavailable; reopen the attachment to try again".into(),
            );
        }
        let index = self
            .attachments
            .iter()
            .position(|path| path == source)
            .ok_or("This attachment is no longer in the draft")?;
        let path = locked(&self.store)?.cache_attachment("annotated.png", bytes)?;
        let mut files = self.attachments.clone();
        files[index] = path.clone();
        vorkspace_core::composer::validate_attachments(&files)?;
        self.attachments = files;
        self.capture_composer(cx);
        match self.composer_draft.flush() {
            Ok(()) => {
                self.composer_draft.error = None;
                cx.notify();
                Ok(path)
            }
            Err(error) => {
                // Restore the visible attachment and pending snapshot. The failed
                // write may also have left a recovery record for user review.
                self.attachments[index] = source.to_path_buf();
                self.capture_composer(cx);
                self.composer_draft.error = Some(format!("Could not save composer: {error}"));
                self.status = format!("Could not save composer: {error}");
                cx.notify();
                Err(error)
            }
        }
    }
    pub(super) fn attachment_chips(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.attachments.is_empty() && self.attachment_error.is_none() && !self.attaching {
            return None;
        }
        Some(
            div()
                .debug_selector(|| "composer-attachments".into())
                .px_3()
                .pt_3()
                .flex()
                .flex_wrap()
                .gap_1p5()
                .children(self.attachments.iter().enumerate().map(|(ix, path)| {
                    let label = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    let image = vorkspace_core::composer::image_mime(path).is_some();
                    let remove = div()
                        .id(("remove-attachment", ix))
                        .role(Role::Button)
                        .aria_label(format!("Remove {label}"))
                        .cursor(CursorStyle::Arrow)
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(image, |d| {
                            d.absolute()
                                .top(px(-4.))
                                .right(px(-4.))
                                .size(px(20.))
                                .rounded_full()
                                .bg(content(0.2))
                                .text_color(content(0.7))
                                .hover(|s| s.bg(content(0.3)).text_color(content(1.)))
                        })
                        .when(!image, |d| {
                            d.h(px(26.))
                                .px(px(6.))
                                .rounded_md()
                                .bg(content(0.1))
                                .text_color(content(1.))
                                .hover(|s| s.bg(content(0.15)))
                        })
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(icon("close", if image { 12. } else { 10. }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            if ix < this.attachments.len() {
                                this.attachments.remove(ix);
                                this.persist_composer(cx);
                                cx.notify();
                            }
                        }));
                    if image {
                        let preview = crate::image_lightbox::Open {
                            source: crate::image_lightbox::Source::Path(path.clone()),
                            alt: label.clone().into(),
                            draft_session: Some(self.summary.id),
                        };
                        return div()
                            .id(("attachment", ix))
                            .relative()
                            .size(px(36.))
                            .flex_shrink_0()
                            .role(Role::Button)
                            .focusable()
                            .debug_selector(move || format!("composer-image-{ix}"))
                            .aria_label(format!("Open {label} full screen"))
                            .cursor(CursorStyle::PointingHand)
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(move |_, window, cx| {
                                cx.stop_propagation();
                                window.dispatch_action(Box::new(preview.clone()), cx);
                            })
                            .child(attachment_thumbnail(path.clone()))
                            .child(remove)
                            .into_any_element();
                    }
                    div()
                        .id(("attachment", ix))
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(content(0.06))
                        .text_size(px(11.))
                        .child(crate::file_icons::file_type_icon(&label, false, false, 14.))
                        .child(div().max_w(px(190.)).truncate().child(label))
                        .child(remove)
                        .into_any_element()
                }))
                .when(self.attaching, |d| {
                    d.child(
                        div()
                            .text_size(px(11.))
                            .text_color(content(0.5))
                            .child("Attaching…"),
                    )
                })
                .when_some(self.attachment_error.clone(), |d, error| {
                    d.child(
                        div()
                            .text_size(px(11.))
                            .text_color(status(StatusColor::Warning, 1.))
                            .child(error),
                    )
                })
                .into_any_element(),
        )
    }
    pub(super) fn finish_orchestration_plan(&mut self, cx: &mut Context<Self>) {
        if !self.planning_orchestration {
            return;
        }
        self.planning_orchestration = false;
        let rows = self.transcript.read(cx).source_rows();
        let start = rows.iter().rposition(|row| row.role == "user").unwrap_or(0);
        let text: String = rows[start..]
            .iter()
            .filter(|row| row.role == "assistant")
            .map(|row| row.text.as_str())
            .collect();
        match vorkspace_core::orchestration::parse(&text) {
            Ok(plan) => self.orchestration_plan = Some(plan),
            Err(error) => self.orchestration_error = Some(error),
        }
    }
    pub(crate) fn set_orchestration_progress(
        &mut self,
        progress: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.orchestration_progress = progress;
        if self.needs_history_restore() {
            self.load(cx);
        }
        cx.notify();
    }
    pub(crate) fn external_outcome(&self, cx: &gpui::App) -> (bool, String) {
        let rows = self.transcript.read(cx).source_rows();
        let start = rows.iter().rposition(|row| row.role == "user").unwrap_or(0);
        let text = vorkspace_core::response::prose(&rows[start..]);
        (
            self.status == "Ready",
            vorkspace_core::response::limit(&format!("{}\n{}", self.status, text), 12_000),
        )
    }
    pub(crate) fn ready_for_external(&self, cx: &gpui::App) -> bool {
        Kind::parse(&self.summary.provider).is_some()
            && self.composer_draft.ready_for_replacement()
            && self.pending_attachments.is_empty()
            && self.history_ready()
            && !self.editing_last_turn.pending()
            && !self.busy
            && !self.stopping
            && !self.preparing_worktree
            && !self.attaching
            && !self.account_switching
            && self.input.read(cx).content.is_empty()
            && self.attachments.is_empty()
    }
    /// Managed runs must wait for history restoration and worktree setup as
    /// well as the provider, including submissions deferred by the editor.
    pub(crate) fn external_submission_pending(&self) -> bool {
        self.busy
            || self.stopping
            || self.preparing_worktree
            || self.attaching
            || self.account_switching
            || self.loading
            || (!self.history_actions.is_empty() && self.history_ready())
            || self.editing_last_turn.pending()
    }
    /// An evicted history can be restored for a managed run; a user's draft
    /// or an already accepted message must keep ownership of this composer.
    pub(crate) fn available_for_managed_run(&self, cx: &gpui::App) -> bool {
        !self.busy
            && !self.stopping
            && !self.preparing_worktree
            && !self.attaching
            && !self.account_switching
            && !self.editing_last_turn.pending()
            && self.input.read(cx).content.is_empty()
            && self.attachments.is_empty()
            && self.follow_ups.state.messages.is_empty()
            && self.follow_ups.state.draft.is_none()
            && !self.settings.worktree_removed
    }
    pub(crate) fn send_coordinator_report(&mut self, prompt: String, cx: &mut Context<Self>) {
        if !self.history_ready() || self.stopping {
            self.defer_history_action(move |this, cx| this.send_coordinator_report(prompt, cx), cx);
            return;
        }
        let internal = self.settings.internal_message;
        self.settings.internal_message = true;
        self.launch_prompt(prompt, cx);
        self.settings.internal_message = internal;
    }
    pub(super) fn orchestration_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.orchestration_plan.is_none()
            && self.orchestration_error.is_none()
            && self.orchestration_progress.is_none()
        {
            return None;
        }
        let mut card = div()
            .id("orchestration-card")
            .mb_2()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(tint(FUCHSIA_300, 0.2))
            .bg(tint(FUCHSIA_300, 0.04))
            .flex()
            .flex_col()
            .gap_2()
            .text_size(px(12.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon("share", 16.).text_color(tint(FUCHSIA_300, 0.8)))
                    .child(
                        self.orchestration_plan
                            .as_ref()
                            .map(|p| p.title.clone())
                            .unwrap_or_else(|| "Orchestrator".into()),
                    ),
            );
        if let Some(plan) = &self.orchestration_plan {
            card = card
                .child(
                    div()
                        .text_color(content(0.6))
                        .child(vorkspace_core::response::limit(&plan.summary, 600)),
                )
                .child(
                    div()
                        .id("orchestration-tasks")
                        .max_h(px(200.))
                        .overflow_y_scroll()
                        .children(plan.tasks.iter().map(|task| {
                            div()
                                .py_1()
                                .child(format!("{} · {}", task.id, task.title))
                                .child(div().text_size(px(10.)).text_color(content(0.4)).child(
                                    format!(
                                        "{}{}",
                                        task.files.join(", "),
                                        if task.depends_on.is_empty() {
                                            String::new()
                                        } else {
                                            format!(" · after {}", task.depends_on.join(", "))
                                        }
                                    ),
                                ))
                        })),
                );
            if self.orchestration_progress.is_none() {
                card = card.child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            tool_pill("start-orchestration", "Start agents")
                                .bg(content(0.1))
                                .child("Start agents")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(plan) = this.orchestration_plan.clone() {
                                        cx.emit(StartOrchestration(plan));
                                    }
                                })),
                        )
                        .child(
                            tool_pill("dismiss-orchestration", "Dismiss plan")
                                .child("Dismiss")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.orchestration_plan = None;
                                    cx.notify();
                                })),
                        ),
                );
            }
        }
        if let Some(error) = &self.orchestration_error {
            card = card
                .child(
                    div()
                        .text_color(status(StatusColor::Warning, 1.))
                        .child(error.clone()),
                )
                .child(
                    tool_pill("dismiss-plan-error", "Dismiss")
                        .child("Dismiss")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.orchestration_error = None;
                            cx.notify();
                        })),
                );
        }
        if let Some(progress) = &self.orchestration_progress {
            card = card
                .child(div().text_color(content(0.6)).child(progress.clone()))
                .child(
                    tool_pill("stop-orchestration", "Stop agents")
                        .child("Stop agents")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(StopOrchestration))),
                );
        }
        Some(card.into_any_element())
    }
}

#[cfg(test)]
mod annotation_tests {
    use super::*;
    use gpui::TestAppContext;
    use vorkspace_core::store::Store;

    #[gpui::test]
    fn failed_draft_commit_restores_attachment_and_retains_recovery(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("vorkspace.db");
        let source = dir.path().join("source.png");
        std::fs::write(&source, b"original").unwrap();
        let store = Store::open_vorkspace(&db_path).unwrap();
        let session = store.create("codex", "/missing/annotation-test").unwrap();
        let id = session.id;
        let db = Arc::new(std::sync::Mutex::new(store));
        let chat = cx.new(|cx| Conversation::new(session, db.clone(), cx));
        cx.run_until_parked();
        let generation = chat.update(cx, |chat, cx| {
            chat.attachments.push(source.clone());
            chat.capture_composer(cx);
            chat.composer_draft.flush().unwrap();
            chat.attachment_draft_generation()
        });
        let external = Store::open_vorkspace(&db_path).unwrap();
        let draft = external.composer_draft(id).unwrap().unwrap();
        external
            .save_composer_draft(id, draft.revision, &draft.payload)
            .unwrap();
        chat.update(cx, |chat, cx| {
            assert!(
                chat.apply_annotated_attachment(&source, generation, b"new image", cx)
                    .is_err()
            );
            assert_eq!(chat.attachments, vec![source.clone()]);
            assert!(chat.composer_draft.error.is_some());
        });
        let draft = external.composer_draft(id).unwrap().unwrap();
        assert_eq!(
            draft.payload["attachments"][0]["path"],
            source.to_str().unwrap()
        );
        assert_eq!(std::fs::read(&source).unwrap(), b"original");
        assert!(!external.composer_recoveries(id).unwrap().is_empty());
    }

    #[gpui::test]
    fn drawing_save_control_publishes_pixels_to_the_draft(cx: &mut TestAppContext) {
        use gpui::{MouseButton, VisualTestContext, point, px};
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.png");
        image::RgbaImage::from_pixel(100, 80, image::Rgba([255, 255, 255, 255]))
            .save(&source)
            .unwrap();
        let original = std::fs::read(&source).unwrap();
        let store = Store::open_vorkspace(&dir.path().join("vorkspace.db")).unwrap();
        let session = store.create("codex", "/missing/annotation-test").unwrap();
        let id = session.id;
        let db = Arc::new(std::sync::Mutex::new(store));
        let owner = cx.new(|cx| Conversation::new(session, db.clone(), cx));
        cx.run_until_parked();
        owner.update(cx, |view, cx| {
            view.attachments.push(source.clone());
            view.capture_composer(cx);
            view.composer_draft.flush().unwrap();
        });
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |window, cx| {
                cx.new(|cx| {
                    crate::image_lightbox::ImageLightbox::new(
                        crate::image_lightbox::Open {
                            source: crate::image_lightbox::Source::Path(source.clone()),
                            alt: "Source".into(),
                            draft_session: Some(id),
                        },
                        Some(owner.clone()),
                        window,
                        cx,
                    )
                })
            })
            .unwrap()
        });
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.refresh().unwrap();
        let brush = visual.debug_bounds("annotation-brush").unwrap().center();
        visual.simulate_click(brush, Default::default());
        let bounds = visual.debug_bounds("lightbox-image").unwrap();
        let start = point(bounds.left() + px(25.), bounds.top() + px(40.));
        let end = point(bounds.left() + px(75.), bounds.top() + px(40.));
        visual.simulate_mouse_down(start, MouseButton::Left, Default::default());
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Default::default());
        visual.simulate_mouse_up(end, MouseButton::Left, Default::default());
        let save = visual.debug_bounds("annotation-save").unwrap().center();
        visual.simulate_click(save, Default::default());
        visual.run_until_parked();
        let path = owner.read_with(visual, |view, _| view.attachments[0].clone());
        assert_ne!(path, source);
        assert_eq!(std::fs::read(&source).unwrap(), original);
        let image = image::open(&path).unwrap().into_rgba8();
        assert_eq!(image.get_pixel(50, 40).0, [255, 59, 48, 255]);
        let draft = locked(&db).unwrap().composer_draft(id).unwrap().unwrap();
        assert_eq!(
            draft.payload["attachments"][0]["path"],
            path.to_str().unwrap()
        );
    }

    #[gpui::test]
    fn close_controls_save_to_original_draft_and_stale_draft_keeps_preview(
        cx: &mut TestAppContext,
    ) {
        use gpui::{VisualTestContext, point, px};
        cx.update(crate::input::bind);
        for (outside, stale) in [(false, false), (true, false), (false, true)] {
            let dir = tempfile::tempdir().unwrap();
            let source = dir.path().join("source.png");
            image::RgbaImage::from_pixel(120, 80, image::Rgba([255, 255, 255, 255]))
                .save(&source)
                .unwrap();
            let original = std::fs::read(&source).unwrap();
            let store = Store::open_vorkspace(&dir.path().join("draft.db")).unwrap();
            let session = store.create("codex", "/missing/close-test").unwrap();
            let id = session.id;
            let db = Arc::new(std::sync::Mutex::new(store));
            let owner = cx.new(|cx| Conversation::new(session, db.clone(), cx));
            cx.run_until_parked();
            owner.update(cx, |view, cx| {
                view.attachments.push(source.clone());
                view.capture_composer(cx);
                view.composer_draft.flush().unwrap();
            });
            let window = cx.update(|cx| {
                cx.open_window(Default::default(), |window, cx| {
                    cx.new(|cx| {
                        crate::image_lightbox::ImageLightbox::new(
                            crate::image_lightbox::Open {
                                source: crate::image_lightbox::Source::Path(source.clone()),
                                alt: "Source".into(),
                                draft_session: Some(id),
                            },
                            Some(owner.clone()),
                            window,
                            cx,
                        )
                    })
                })
                .unwrap()
            });
            let visual = &mut VisualTestContext::from_window(window.into(), cx);
            visual.run_until_parked();
            visual.refresh().unwrap();
            let brush = visual.debug_bounds("annotation-brush").unwrap().center();
            visual.simulate_click(brush, Default::default());
            let bounds = visual.debug_bounds("lightbox-image").unwrap();
            visual.simulate_click(bounds.center(), Default::default());
            let text_tool = visual.debug_bounds("annotation-text").unwrap().center();
            visual.simulate_click(text_tool, Default::default());
            visual.simulate_click(
                point(bounds.left() + px(10.), bounds.top() + px(10.)),
                Default::default(),
            );
            visual.simulate_input("OK");
            if stale {
                owner.update(visual, |view, cx| {
                    view.input
                        .update(cx, |input, cx| input.set_text("Changed draft".into(), cx));
                });
            }
            let target = if outside {
                point(px(4.), px(4.))
            } else {
                visual.debug_bounds("close-image-preview").unwrap().center()
            };
            visual.simulate_click(target, Default::default());
            // Repeated close requests cannot launch another publication.
            visual.simulate_click(target, Default::default());
            visual.run_until_parked();
            visual.refresh().unwrap();
            let attachment = owner.read_with(visual, |view, _| view.attachments[0].clone());
            assert_eq!(std::fs::read(&source).unwrap(), original);
            if stale {
                assert_eq!(attachment, source);
                assert!(visual.debug_bounds("annotation-error").is_some());
                assert!(visual.debug_bounds("lightbox-image").is_some());
                assert!(visual.debug_bounds("annotation-text-box-0").is_some());
            } else {
                assert_ne!(attachment, source);
                assert_eq!(
                    image::open(&attachment)
                        .unwrap()
                        .into_rgba8()
                        .get_pixel(60, 40)
                        .0,
                    [255, 59, 48, 255]
                );
                let flattened = image::open(&attachment).unwrap().into_rgba8();
                assert!((10..35).any(|y| {
                    (10..70).any(|x| flattened.get_pixel(x, y).0 != [255, 255, 255, 255])
                }));
                let draft = locked(&db).unwrap().composer_draft(id).unwrap().unwrap();
                assert_eq!(
                    draft.payload["attachments"][0]["path"],
                    attachment.to_str().unwrap()
                );
            }
            window
                .update(visual, |_, window, _| window.remove_window())
                .unwrap();
        }
    }

    #[gpui::test]
    fn annotated_copy_is_persisted_and_sent_and_stale_owner_is_rejected(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("vorkspace.db");
        let source = dir.path().join("source.png");
        image::RgbaImage::from_pixel(8, 8, image::Rgba([255, 255, 255, 255]))
            .save(&source)
            .unwrap();
        let original = std::fs::read(&source).unwrap();
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([255, 59, 48, 255]),
        ))
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
        let bytes = encoded.into_inner();
        let store = Store::open_vorkspace(&db_path).unwrap();
        let session = store.create("codex", "/missing/annotation-test").unwrap();
        let id = session.id;
        let db = Arc::new(std::sync::Mutex::new(store));
        let chat = cx.new(|cx| Conversation::new(session.clone(), db.clone(), cx));
        cx.run_until_parked();
        let annotated = chat.update(cx, |chat, cx| {
            chat.attachments.push(source.clone());
            chat.input
                .update(cx, |input, cx| input.set_text("Keep this draft".into(), cx));
            chat.capture_composer(cx);
            chat.composer_draft.flush().unwrap();
            let generation = chat.attachment_draft_generation();
            let too_large = vec![0; vorkspace_core::composer::MAX_EMBED_BYTES as usize + 1];
            assert!(
                chat.apply_annotated_attachment(&source, generation, &too_large, cx)
                    .is_err()
            );
            assert_eq!(chat.attachments, vec![source.clone()]);
            let path = chat
                .apply_annotated_attachment(&source, generation, &bytes, cx)
                .unwrap();
            assert!(
                chat.apply_annotated_attachment(&source, generation, b"stale", cx)
                    .is_err()
            );
            path
        });
        assert_eq!(std::fs::read(&source).unwrap(), original);
        assert_eq!(std::fs::read(&annotated).unwrap(), bytes);
        let wire_image = vorkspace_core::composer::image_data(&annotated).unwrap();
        assert_eq!(
            vorkspace_core::composer::decode_image_data(&wire_image).unwrap(),
            bytes
        );
        let draft = locked(&db).unwrap().composer_draft(id).unwrap().unwrap();
        assert_eq!(
            draft.payload["attachments"][0]["path"],
            annotated.to_str().unwrap()
        );
        let reopened = cx.new(|cx| Conversation::new(session, db.clone(), cx));
        cx.run_until_parked();
        reopened.update(cx, |chat, cx| {
            assert_eq!(chat.attachments, vec![annotated.clone()]);
            assert_eq!(chat.input.read(cx).content, "Keep this draft");
        });
        drop(reopened);
        chat.update(cx, |chat, cx| {
            chat.loaded = true;
            chat.send(cx);
            let rows = chat.transcript.read(cx).source_rows();
            let manifest = rows
                .iter()
                .find(|row| row.role == vorkspace_core::transcript::attachments::ROLE)
                .unwrap();
            assert_eq!(
                vorkspace_core::transcript::attachments::paths(manifest).unwrap(),
                &[annotated]
            );
        });
    }
}
