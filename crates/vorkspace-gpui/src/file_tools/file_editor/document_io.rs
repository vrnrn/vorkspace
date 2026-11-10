//! Admission, observation and save for a file tab.
use super::*;

impl FileEditor {
    pub(super) fn load(&mut self, force: bool, cx: &mut Context<Self>) {
        if self.saving || (!force && self.load_task.is_some()) {
            return;
        }
        let requested_from = force.then(|| self.input.read(cx).content.clone());
        let path = PathBuf::from(&self.path);
        let known = self.disk_revision;
        let work = cx.background_executor().spawn(async move {
            let revision = document::local_revision(&path);
            if !force && revision.is_some() && revision == known {
                return Ok(None);
            }
            let result = document::read_document(&path).and_then(media::prepare_document);
            if revision.is_some() && revision != document::local_revision(&path) {
                return Ok(None);
            }
            result.map(|document| Some((revision, document)))
        });
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.load_task = None;
                let (revision, document) = match result {
                    Ok(Some(loaded)) => loaded,
                    Ok(None) => return,
                    Err(error) => {
                        this.disk_revision = None;
                        if this.load_error.as_ref() != Some(&error) {
                            this.load_error = Some(error);
                            cx.notify();
                        }
                        return;
                    }
                };
                let revision_changed = revision.is_some() && this.disk_revision != revision;
                this.disk_revision = revision;
                let recovered = this.load_error.take().is_some();
                // Confirmation discarded the draft present when reload began.
                // Typing after that request still belongs to the user.
                let discard = requested_from.as_ref() == Some(&this.input.read(cx).content);
                match document {
                    Document::Text(doc) => {
                        if !force
                            && this
                                .document
                                .as_ref()
                                .is_some_and(|old| old.disk == doc.disk)
                        {
                            let conflict = std::mem::take(&mut this.disk_changed);
                            if conflict {
                                this.error = None;
                                this.schedule_autosave(cx);
                            }
                            if conflict || recovered {
                                cx.notify();
                            }
                            return;
                        }
                        if this.dirty && !discard {
                            this.disk_changed = true;
                            cx.notify();
                            return;
                        }
                        this.error = None;
                        this.disk_changed = false;
                        this.image = None;
                        this.media = None;
                        this.media_events = None;
                        this.unsupported = None;
                        let first = this.document.is_none();
                        let text = doc.text.clone();
                        this.document = Some(doc);
                        this.input.update(cx, |input, cx| {
                            input.reload_file(text, !first && !discard, cx);
                        });
                        this.dirty = false;
                        if first && (this.is_markdown() || this.is_svg()) {
                            this.preview = cx
                                .try_global::<vorkspace_ui::preferences::Preferences>()
                                .is_none_or(|prefs| prefs.file_view_mode != "source");
                        }
                        this.apply_navigation(cx);
                        this.update_preview(cx);
                        this.schedule_analysis(cx);
                        cx.emit(FileEvent::Dirty);
                    }
                    Document::Image {
                        bytes,
                        format,
                        width,
                        height,
                        original_size,
                    } => {
                        if this.dirty && !discard {
                            this.disk_changed = true;
                            cx.notify();
                            return;
                        }
                        let byte_size = original_size;
                        let file_format = if format == "tiff" { "avif" } else { format };
                        let format = match format {
                            "jpeg" => ImageFormat::Jpeg,
                            "webp" => ImageFormat::Webp,
                            "gif" => ImageFormat::Gif,
                            "bmp" => ImageFormat::Bmp,
                            "ico" => ImageFormat::Ico,
                            "tiff" => ImageFormat::Tiff,
                            _ => ImageFormat::Png,
                        };
                        let image = Arc::new(Image::from_bytes(format, bytes));
                        if this
                            .image
                            .as_ref()
                            .is_some_and(|old| old.id() == image.id())
                        {
                            if recovered {
                                cx.notify();
                            }
                            return;
                        }
                        this.media = Some(cx.new(|_| {
                            let mut preview = media::MediaPreview::image(
                                this.path.clone(),
                                image.clone(),
                                width,
                                height,
                                byte_size,
                                file_format,
                            );
                            preview.set_visible(this.visible);
                            preview
                        }));
                        this.image = Some(image);
                        this.media_events = None;
                        this.dirty = false;
                        this.disk_changed = false;
                        cx.emit(FileEvent::Dirty);
                        this.document = None;
                        this.unsupported = None;
                        this.error = None;
                    }
                    Document::Video {
                        size,
                        modified,
                        source,
                    } => {
                        if this.dirty && !discard {
                            this.disk_changed = true;
                            cx.notify();
                            return;
                        }
                        let same = this
                            .media
                            .as_ref()
                            .is_some_and(|m| m.read(cx).same_video(size, modified));
                        if !same || force || revision_changed {
                            this.media = Some(cx.new(|cx| {
                                media::MediaPreview::video(
                                    this.path.clone(),
                                    size,
                                    modified,
                                    source,
                                    cx,
                                )
                            }));
                        }
                        if let Some(media) = &this.media {
                            media.update(cx, |m, _| m.set_visible(this.visible));
                            this.media_events =
                                Some(cx.subscribe(media, |this, _, _: &media::MediaEvent, cx| {
                                    this.load(true, cx)
                                }));
                        }
                        this.document = None;
                        this.image = None;
                        this.unsupported = None;
                        this.error = None;
                        this.dirty = false;
                        this.disk_changed = false;
                        cx.emit(FileEvent::Dirty);
                    }
                    Document::Unsupported(reason) => {
                        if this.dirty && !discard {
                            this.disk_changed = true;
                            cx.notify();
                            return;
                        }
                        this.media = None;
                        this.media_events = None;
                        this.unsupported = Some(reason);
                        this.disk_changed = false;
                        this.document = None;
                        this.image = None;
                        this.error = None;
                        this.dirty = false;
                        cx.emit(FileEvent::Dirty);
                    }
                }
                cx.notify();
            });
        }));
    }
    pub(super) fn schedule_autosave(&mut self, cx: &mut Context<Self>) {
        self.autosave = None;
        if !self.dirty || self.disk_changed || !self.autosave_enabled {
            return;
        }
        self.autosave = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let _ = this.update(cx, |this, cx| {
                if this.dirty && !this.disk_changed && this.autosave_enabled {
                    if this.input.read(cx).is_composing() || this.saving {
                        this.schedule_autosave(cx);
                    } else {
                        this.save(cx);
                    }
                }
            });
        }));
    }
    pub(crate) fn save(&mut self, cx: &mut Context<Self>) {
        if self.saving || self.input.read(cx).is_composing() || self.document.is_none() {
            return;
        }
        self.load_task = None;
        let source = self.input.read(cx).content.clone();
        let at = self.input.read(cx).selection_range().end;
        let path = self.path.clone();
        let format = cx
            .try_global::<vorkspace_ui::preferences::Preferences>()
            .is_none_or(|p| p.format_on_save);
        self.saving = true;
        self.error = None;
        cx.spawn(async move |this, cx| {
            if format {
                let text = source.clone();
                let result = cx
                    .background_executor()
                    .spawn(
                        async move { vorkspace_editor::editor_analysis::format(&path, &text, at) },
                    )
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if this.input.read(cx).content == source
                        && !this.input.read(cx).is_composing()
                        && let Ok(Some(result)) = result
                        && result.formatted != source.as_ref()
                    {
                        this.input.update(cx, |input, cx| {
                            input.replace_bytes(
                                0..source.len(),
                                &result.formatted,
                                result.cursor,
                                cx,
                            )
                        });
                    }
                });
            }
            let Ok(Some((path, doc, saved))) = this.update(cx, |this, cx| {
                if this.input.read(cx).is_composing() {
                    this.saving = false;
                    this.schedule_autosave(cx);
                    cx.emit(FileEvent::SaveFailed);
                    return None;
                }
                Some((
                    PathBuf::from(&this.path),
                    this.document.clone()?,
                    this.input.read(cx).content.to_string(),
                ))
            }) else {
                return;
            };
            let text = saved.clone();
            let result = cx
                .background_executor()
                .spawn(async move { document::save_document(&path, &doc, &text) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.saving = false;
                match result {
                    Ok(disk) => {
                        if let Some(doc) = &mut this.document {
                            doc.disk = disk;
                            doc.text = saved;
                        }
                        this.disk_changed = false;
                        this.load_error = None;
                        this.edited(cx);
                        this.changed_checkout(cx);
                        cx.emit(FileEvent::Saved);
                    }
                    Err(error) => {
                        this.disk_changed |= error.contains("changed on disk");
                        this.error = Some(error);
                        cx.emit(FileEvent::SaveFailed);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.dirty {
            self.load(true, cx);
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            "Discard your edits and reload this file?",
            None,
            &["Reload", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if matches!(answer.await, Ok(0)) {
                let _ = this.update(cx, |this, cx| this.load(true, cx));
            }
        })
        .detach();
    }
    pub(super) fn save_my_changes(&mut self, cx: &mut Context<Self>) {
        if self.saving || self.document.is_none() {
            return;
        }
        self.load_task = None;
        self.saving = true;
        let path = PathBuf::from(&self.path);
        let work = cx.background_executor().spawn(async move {
            match document::read_document(&path)? {
                Document::Text(document) => Ok(document),
                _ => Err("The file is no longer a text document. Your edits were kept.".into()),
            }
        });
        cx.spawn(async move |this, cx| {
            let latest = work.await;
            let _ = this.update(cx, |this, cx| {
                this.saving = false;
                match latest {
                    Ok(document) => {
                        // Consent applies to this observed disk revision only.
                        // The normal save still checks for subsequent changes.
                        this.document = Some(document);
                        this.disk_changed = false;
                        this.save(cx);
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
