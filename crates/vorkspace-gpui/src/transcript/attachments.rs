use super::*;
use gpui::{AnyElement, ImageSource};
use std::path::PathBuf;

#[derive(Clone)]
pub(crate) struct Attachment {
    name: SharedString,
    path: Option<PathBuf>,
    mime: SharedString,
    image: bool,
    data: Option<Arc<str>>,
}

impl Attachment {
    fn from_value(mut value: serde_json::Value) -> Self {
        let path = value["path"]
            .as_str()
            .filter(|p| !p.is_empty())
            .map(PathBuf::from);
        let name = value["name"]
            .as_str()
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .or_else(|| path.as_ref()?.file_name()?.to_str().map(str::to_owned))
            .unwrap_or_else(|| "Image".into());
        let mime = value["mimeType"].as_str().unwrap_or("").to_owned();
        let image = value["kind"] == "image"
            || mime.starts_with("image/")
            || path
                .as_ref()
                .is_some_and(|path| vorkspace_core::composer::image_mime(path).is_some());
        let data = match value["data"].take() {
            serde_json::Value::String(data) if !data.is_empty() => Some(Arc::from(data)),
            _ => path
                .as_ref()
                .and_then(|path| path.to_str())
                .filter(|path| vorkspace_core::remote::attachments::is_attachment_source(path))
                .map(Arc::from),
        };
        Self {
            name: name.into(),
            path,
            mime: mime.into(),
            image,
            data,
        }
    }

    pub(super) fn from_paths(paths: &[PathBuf]) -> Arc<[Self]> {
        let value = vorkspace_core::transcript::attachments::from_paths(paths);
        value
            .as_array()
            .into_iter()
            .flatten()
            .cloned()
            .map(Self::from_value)
            .collect()
    }
}

pub(crate) fn message(rows: &[Row], end: usize, text: &str) -> (Option<String>, Arc<[Attachment]>) {
    if let Some(manifest) = rows
        .get(end)
        .filter(|row| row.role == vorkspace_core::transcript::attachments::ROLE)
    {
        let files: Vec<serde_json::Value> =
            serde_json::from_str(&manifest.text).unwrap_or_default();
        return (
            None,
            files.into_iter().map(Attachment::from_value).collect(),
        );
    }
    let (body, paths) = vorkspace_core::transcript::attachments::legacy_tail(text);
    (
        (!paths.is_empty()).then(|| body.to_owned()),
        Attachment::from_paths(&paths),
    )
}

/// Embedded images are decoded only when their bubble enters the viewport.
/// File images use GPUI's asynchronous resource cache directly.
pub(super) struct Photo {
    data: Arc<str>,
    bytes: usize,
    mime: SharedString,
    source: Option<ImageSource>,
}

impl Photo {
    fn new(data: Arc<str>, mime: SharedString, cx: &mut Context<Self>) -> Self {
        let encoded = data.clone();
        let format = match mime.as_ref() {
            "image/jpeg" | "image/jpg" => gpui::ImageFormat::Jpeg,
            "image/gif" => gpui::ImageFormat::Gif,
            "image/webp" => gpui::ImageFormat::Webp,
            "image/svg+xml" => gpui::ImageFormat::Svg,
            "image/bmp" => gpui::ImageFormat::Bmp,
            "image/tiff" => gpui::ImageFormat::Tiff,
            _ => gpui::ImageFormat::Png,
        };
        let work = cx.background_executor().spawn(async move {
            vorkspace_core::composer::decode_image_data(&encoded)
                .map(|bytes| Arc::new(gpui::Image::from_bytes(format, bytes)))
        });
        cx.spawn(async move |this, cx| {
            let source = work.await.ok().map(ImageSource::from);
            let _ = this.update(cx, |this, cx| {
                this.source = source;
                cx.notify();
            });
        })
        .detach();
        Self {
            bytes: vorkspace_core::remote::attachments::attachment_source_size(&data)
                .unwrap_or(data.len()),
            data,
            mime,
            source: None,
        }
    }
}

impl Render for Photo {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        if let Some(source) = &self.source {
            attachment_thumbnail(source.clone())
        } else {
            div()
                .size(px(36.))
                .flex_shrink_0()
                .rounded_lg()
                .bg(content(0.04))
                .flex()
                .items_center()
                .justify_center()
                .child(icon("file", 14.))
                .into_any_element()
        }
    }
}

impl TranscriptView {
    pub(super) fn attachment_row(
        &mut self,
        files: &[Attachment],
        key: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_wrap()
            .gap_1p5()
            .children(files.iter().enumerate().map(|(ix, file)| {
                if file.image {
                    let photo = if let Some(data) = &file.data {
                        let key = format!("{key}-{ix}");
                        let cached = self
                            .attachment_images
                            .get(&key)
                            .filter(|photo| {
                                let photo = photo.read(cx);
                                (Arc::ptr_eq(&photo.data, data)
                                    || (vorkspace_core::remote::attachments::is_attachment_source(
                                        data,
                                    ) && photo.data.as_ref() == data.as_ref()))
                                    && photo.mime == file.mime
                            })
                            .cloned();
                        let photo = cached.unwrap_or_else(|| {
                            let photo =
                                cx.new(|cx| Photo::new(data.clone(), file.mime.clone(), cx));
                            self.attachment_images.insert(key.clone(), photo.clone());
                            // Bound decoded image ownership while scrolling long histories.
                            while self.attachment_images.len() > 32
                                || self
                                    .attachment_images
                                    .values()
                                    .map(|photo| photo.read(cx).bytes)
                                    .sum::<usize>()
                                    > 64 * 1024 * 1024
                            {
                                let Some(old) = self
                                    .attachment_images
                                    .keys()
                                    .find(|old| **old != key)
                                    .cloned()
                                else {
                                    break;
                                };
                                self.attachment_images.remove(&old);
                            }
                            photo
                        });
                        photo.into_any_element()
                    } else if let Some(path) = &file.path {
                        attachment_thumbnail(path.clone())
                    } else {
                        icon("file", 16.).into_any_element()
                    };
                    let source = file
                        .data
                        .as_ref()
                        .map(|data| crate::image_lightbox::Source::Embedded {
                            data: data.clone(),
                            mime: file.mime.clone(),
                        })
                        .or_else(|| file.path.clone().map(crate::image_lightbox::Source::Path));
                    let alt = file.name.clone();
                    div()
                        .id(SharedString::from(format!("attachment-{key}-{ix}")))
                        .size(px(36.))
                        .flex_shrink_0()
                        .role(Role::Button)
                        .focusable()
                        .debug_selector(move || format!("transcript-image-{ix}"))
                        .aria_label(format!("Open {} full screen", file.name))
                        .when_some(source, |d, source| {
                            d.cursor(gpui::CursorStyle::PointingHand)
                                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation()
                                })
                                .on_click(move |_, window, cx| {
                                    cx.stop_propagation();
                                    window.dispatch_action(
                                        Box::new(crate::image_lightbox::Open {
                                            source: source.clone(),
                                            alt: alt.clone(),
                                        }),
                                        cx,
                                    );
                                })
                        })
                        .child(photo)
                        .into_any_element()
                } else {
                    div()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .px_1()
                        .py_1()
                        .rounded_md()
                        .bg(content(0.06))
                        .child(crate::file_icons::file_type_icon(
                            &file.name, false, false, 16.,
                        ))
                        .child(
                            div()
                                .max_w(px(140.))
                                .truncate()
                                .text_size(px(11.))
                                .child(file.name.clone()),
                        )
                        .into_any_element()
                }
            }))
            .into_any_element()
    }
}
