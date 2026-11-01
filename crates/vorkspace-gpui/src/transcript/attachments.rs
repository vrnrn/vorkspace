use super::*;
use gpui::{AnyElement, Entity, ImageSource, RenderImage};
use image::{AnimationDecoder, Frame, ImageDecoder, ImageReader, Limits};
use std::{
    fs::File,
    io::{Cursor, Read},
    path::PathBuf,
};

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
    loading: bool,
    mime: SharedString,
    image: Option<Arc<RenderImage>>,
}

impl Photo {
    fn decoded(&mut self, image: Option<Arc<RenderImage>>, cx: &mut Context<Self>) {
        self.image = image;
        self.loading = false;
        cx.notify();
    }
    fn matches_source(&self, data: &Arc<str>, mime: &SharedString) -> bool {
        (Arc::ptr_eq(&self.data, data) || self.data.as_ref() == data.as_ref()) && &self.mime == mime
    }

    fn retained_bytes(&self) -> usize {
        let pixels = self.image.as_ref().map_or(
            if self.loading { MAX_THUMBNAIL_BYTES } else { 0 },
            |image| {
                (0..image.frame_count())
                    .filter_map(|index| image.as_bytes(index))
                    .map(<[u8]>::len)
                    .sum()
            },
        );
        self.data.len().saturating_add(pixels)
    }

    fn new(data: Arc<str>, mime: SharedString, cx: &mut Context<Self>) -> Self {
        let encoded = data.clone();
        let format = mime.clone();
        let work = cx.background_executor().spawn(async move {
            if encoded.len() > MAX_ATTACHMENT_SOURCE_BYTES {
                return Err("attachment image exceeds the encoded size limit".into());
            }
            let bytes = vorkspace_core::composer::decode_image_data(&encoded)
                .map_err(|error| error.to_string())?;
            decode_attachment_thumbnail(&bytes, &format)
        });
        cx.spawn(async move |this, cx| {
            let image = work.await.ok();
            let _ = this.update(cx, |this, cx| this.decoded(image, cx));
        })
        .detach();
        Self {
            loading: true,
            data,
            mime,
            image: None,
        }
    }

    fn from_path(path: PathBuf, mime: SharedString, cx: &mut Context<Self>) -> Self {
        let display_path: Arc<str> = Arc::from(path.to_string_lossy().as_ref());
        let decode_mime = mime.clone();
        let work = cx.background_executor().spawn(async move {
            let bytes = read_attachment_file(path)?;
            decode_attachment_thumbnail(&bytes, &decode_mime)
        });
        cx.spawn(async move |this, cx| {
            let image = work.await.ok();
            let _ = this.update(cx, |this, cx| this.decoded(image, cx));
        })
        .detach();
        Self {
            loading: true,
            data: display_path,
            mime,
            image: None,
        }
    }
}

const THUMBNAIL_MAX_SIDE: u32 = 108;
const MAX_ATTACHMENT_SOURCE_BYTES: usize = 64 * 1024 * 1024;
const MAX_DECODED_DIMENSION: u32 = 32_768;
const MAX_DECODED_PIXELS: u64 = 32 * 1024 * 1024;
const MAX_DECODE_ALLOCATION: u64 = MAX_DECODED_PIXELS * 4;
const MAX_GIF_THUMBNAIL_FRAMES: usize = 120;
const MAX_THUMBNAIL_BYTES: usize =
    THUMBNAIL_MAX_SIDE as usize * THUMBNAIL_MAX_SIDE as usize * 4 * MAX_GIF_THUMBNAIL_FRAMES;
const THUMBNAIL_CACHE_BYTES: usize = 64 * 1024 * 1024;

fn read_attachment_file(path: PathBuf) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|error| error.to_string())?;
    if file.metadata().map_err(|error| error.to_string())?.len()
        > MAX_ATTACHMENT_SOURCE_BYTES as u64
    {
        return Err("attachment image exceeds the encoded size limit".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_ATTACHMENT_SOURCE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > MAX_ATTACHMENT_SOURCE_BYTES {
        return Err("attachment image exceeds the encoded size limit".into());
    }
    Ok(bytes)
}

fn thumbnail_limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DECODED_DIMENSION);
    limits.max_image_height = Some(MAX_DECODED_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_ALLOCATION);
    limits
}

fn check_decode_dimensions(width: u32, height: u32) -> Result<(), String> {
    if width > MAX_DECODED_DIMENSION
        || height > MAX_DECODED_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_DECODED_PIXELS
    {
        return Err("attachment image dimensions exceed the decode limit".into());
    }
    Ok(())
}

fn thumbnail_frame(mut frame: Frame) -> Frame {
    let (width, height) = frame.buffer().dimensions();
    if width.max(height) > THUMBNAIL_MAX_SIDE {
        let buffer =
            image::imageops::thumbnail(frame.buffer(), THUMBNAIL_MAX_SIDE, THUMBNAIL_MAX_SIDE);
        let scale = THUMBNAIL_MAX_SIDE as f32 / width.max(height) as f32;
        frame = Frame::from_parts(
            buffer,
            (frame.left() as f32 * scale).round() as u32,
            (frame.top() as f32 * scale).round() as u32,
            frame.delay(),
        );
    }
    for pixel in frame.buffer_mut().as_mut().as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    frame
}

fn decode_attachment_thumbnail(bytes: &[u8], mime: &str) -> Result<Arc<RenderImage>, String> {
    let declared_format = match mime {
        "image/jpeg" | "image/jpg" => image::ImageFormat::Jpeg,
        "image/gif" => image::ImageFormat::Gif,
        "image/webp" => image::ImageFormat::WebP,
        "image/bmp" => image::ImageFormat::Bmp,
        "image/tiff" => image::ImageFormat::Tiff,
        "image/x-icon" => image::ImageFormat::Ico,
        _ => image::ImageFormat::Png,
    };
    if mime == "image/svg+xml" {
        // Rasterize at thumbnail width. GPUI caps the other axis at 8192px;
        // the resulting temporary buffer is bounded before we reduce both
        // dimensions to the retained 108px maximum.
        let renderer = gpui::SvgRenderer::new(Arc::new(()));
        let svg = renderer
            .parse_svg(bytes)
            .map_err(|error| error.to_string())?;
        let image = renderer
            .render_parsed(
                &svg,
                gpui::SvgSize::Size(gpui::size(
                    gpui::DevicePixels(THUMBNAIL_MAX_SIDE as i32),
                    gpui::DevicePixels(THUMBNAIL_MAX_SIDE as i32),
                )),
            )
            .map_err(|error| error.to_string())?;
        return Ok(Arc::new(RenderImage::new(
            (0..image.frame_count())
                .filter_map(|index| render_image_frame(&image, index))
                .map(thumbnail_frame)
                .collect::<Vec<_>>(),
        )));
    }

    let image_format = image::guess_format(bytes).unwrap_or(declared_format);
    if image_format == image::ImageFormat::Gif {
        let mut decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes))
            .map_err(|error| error.to_string())?;
        decoder
            .set_limits(thumbnail_limits())
            .map_err(|error| error.to_string())?;
        let (width, height) = decoder.dimensions();
        check_decode_dimensions(width, height)?;
        return bounded_animation(decoder.into_frames());
    }
    if image_format == image::ImageFormat::WebP {
        let mut decoder = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes))
            .map_err(|error| error.to_string())?;
        decoder
            .set_limits(thumbnail_limits())
            .map_err(|error| error.to_string())?;
        let (width, height) = decoder.dimensions();
        check_decode_dimensions(width, height)?;
        if decoder.has_animation() {
            return bounded_animation(decoder.into_frames());
        }
        let orientation = decoder.orientation().map_err(|error| error.to_string())?;
        let mut image =
            image::DynamicImage::from_decoder(decoder).map_err(|error| error.to_string())?;
        image.apply_orientation(orientation);
        return Ok(Arc::new(RenderImage::new(vec![thumbnail_frame(
            Frame::new(image.into_rgba8()),
        )])));
    }

    let mut decoder = ImageReader::with_format(Cursor::new(bytes), image_format)
        .into_decoder()
        .map_err(|error| error.to_string())?;
    decoder
        .set_limits(thumbnail_limits())
        .map_err(|error| error.to_string())?;
    let (width, height) = decoder.dimensions();
    check_decode_dimensions(width, height)?;
    let orientation = decoder.orientation().map_err(|error| error.to_string())?;
    let mut image =
        image::DynamicImage::from_decoder(decoder).map_err(|error| error.to_string())?;
    image.apply_orientation(orientation);
    let frame = Frame::new(image.into_rgba8());
    Ok(Arc::new(RenderImage::new(vec![thumbnail_frame(frame)])))
}

fn bounded_animation(
    frames: impl Iterator<Item = image::ImageResult<Frame>>,
) -> Result<Arc<RenderImage>, String> {
    let frames = frames
        .take(MAX_GIF_THUMBNAIL_FRAMES)
        .filter_map(Result::ok)
        .map(thumbnail_frame)
        .collect::<Vec<_>>();
    if frames.is_empty() {
        return Err("animated image contains no decodable frames".into());
    }
    Ok(Arc::new(RenderImage::new(frames)))
}

fn render_image_frame(image: &RenderImage, index: usize) -> Option<Frame> {
    let size = image.size(index);
    let bytes = image.as_bytes(index)?;
    let mut buffer =
        image::RgbaImage::from_raw(size.width.0 as u32, size.height.0 as u32, bytes.to_vec())?;
    // GPUI RenderImage storage is BGRA; image::Frame expects RGBA.
    for pixel in buffer.as_mut().as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Some(Frame::from_parts(buffer, 0, 0, image.delay(index)))
}

impl Render for Photo {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        if let Some(image) = &self.image {
            attachment_thumbnail(ImageSource::Render(image.clone()))
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
    fn cache_photo(&mut self, key: String, photo: Entity<Photo>, cx: &mut Context<Self>) {
        // Reserve the decoder's maximum retained output before it starts. A
        // pending Photo can never grow beyond its reservation after publication.
        // An oversized source stays visible without evicting the entire cache.
        if photo.read(cx).retained_bytes() > THUMBNAIL_CACHE_BYTES {
            return;
        }
        self.attachment_images.insert(key.clone(), photo);
        while self.attachment_images.len() > 32
            || self
                .attachment_images
                .values()
                .map(|p| p.read(cx).retained_bytes())
                .sum::<usize>()
                > THUMBNAIL_CACHE_BYTES
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
    }
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
                    let image_source =
                        file.data
                            .as_ref()
                            .map(|data| (data.clone(), None))
                            .or_else(|| {
                                file.path.as_ref().map(|path| {
                                    (
                                        Arc::from(path.to_string_lossy().as_ref()),
                                        Some(path.clone()),
                                    )
                                })
                            });
                    let photo = if let Some((data, path)) = image_source {
                        let key = format!("{key}-{ix}");
                        let mime = if file.mime.is_empty() {
                            path.as_ref()
                                .and_then(|path| vorkspace_core::composer::image_mime(path))
                                .unwrap_or("image/png")
                                .into()
                        } else {
                            file.mime.clone()
                        };
                        let cached = self
                            .attachment_images
                            .get(&key)
                            .filter(|photo| photo.read(cx).matches_source(&data, &mime))
                            .cloned();
                        let photo = cached.unwrap_or_else(|| {
                            let photo = cx.new(|cx| {
                                let photo = if let Some(path) = path {
                                    Photo::from_path(path, mime.clone(), cx)
                                } else {
                                    Photo::new(data.clone(), mime.clone(), cx)
                                };
                                cx.on_release(|photo, cx| {
                                    if let Some(image) = photo.image.take() {
                                        cx.drop_image(image, None);
                                    }
                                })
                                .detach();
                                photo
                            });
                            self.cache_photo(key, photo.clone(), cx);
                            photo
                        });
                        photo.into_any_element()
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
                                            draft_session: None,
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
                        .child(vorkspace_ui::file_icons::file_type_icon(
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

#[cfg(test)]
mod tests;
