//! Native port of `src/shared/ui/ImageLightbox.tsx`.
mod annotation;
use gpui::{
    App, Bounds, BoxShadow, Context, CursorStyle, Entity, EventEmitter, FocusHandle, Focusable,
    Image, ImageFormat, ImgResourceLoader, KeyBinding, MouseButton, Pixels, Point, RenderImage,
    Role, SharedString, Size, Window, actions, canvas, div, img, point, prelude::*, px, size,
};
use std::time::Instant;
use std::{path::PathBuf, sync::Arc};
use vorkspace_ui::theme::{black, icon, white};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Source {
    Path(PathBuf),
    Embedded { data: Arc<str>, mime: SharedString },
}

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = image_lightbox, no_json)]
pub(crate) struct Open {
    pub source: Source,
    pub alt: SharedString,
    pub draft_session: Option<i64>,
}

actions!(image_lightbox, [Close, ZoomIn, ZoomOut, ResetZoom, Save]);

pub(crate) fn bind(cx: &mut App) {
    for modifier in ["cmd", "ctrl"] {
        cx.bind_keys([
            KeyBinding::new(&format!("{modifier}-+"), ZoomIn, Some("ImageLightbox")),
            KeyBinding::new(&format!("{modifier}-="), ZoomIn, Some("ImageLightbox")),
            KeyBinding::new(&format!("{modifier}--"), ZoomOut, Some("ImageLightbox")),
            KeyBinding::new(&format!("{modifier}-0"), ResetZoom, Some("ImageLightbox")),
        ]);
    }
    cx.bind_keys([
        KeyBinding::new("escape", Close, Some("ImageLightbox")),
        KeyBinding::new("cmd-s", Save, Some("ImageLightbox")),
    ]);
}

const MIN_SCALE: f32 = 1.;
const MAX_SCALE: f32 = 8.;
const DOUBLE_CLICK_SCALE: f32 = 2.5;
const KEY_ZOOM_STEP: f32 = 1.25;

#[derive(Clone, Copy, Debug, PartialEq)]
struct View {
    scale: f32,
    x: f32,
    y: f32,
}
impl View {
    const IDENTITY: Self = Self {
        scale: 1.,
        x: 0.,
        y: 0.,
    };

    fn clamp(self, viewport: Size<Pixels>, image: Size<Pixels>) -> Self {
        if self.scale <= MIN_SCALE {
            return Self::IDENTITY;
        }
        let max_x = ((image.width.as_f32() * self.scale - viewport.width.as_f32()) / 2.).max(0.);
        let max_y = ((image.height.as_f32() * self.scale - viewport.height.as_f32()) / 2.).max(0.);
        Self {
            x: self.x.clamp(-max_x, max_x),
            y: self.y.clamp(-max_y, max_y),
            ..self
        }
    }

    fn zoom_at(
        self,
        scale: f32,
        pointer: Point<Pixels>,
        viewport: Size<Pixels>,
        image: Size<Pixels>,
    ) -> Self {
        let scale = scale.clamp(MIN_SCALE, MAX_SCALE);
        let dx = pointer.x.as_f32() - viewport.width.as_f32() / 2.;
        let dy = pointer.y.as_f32() - viewport.height.as_f32() / 2.;
        let ratio = scale / self.scale;
        Self {
            scale,
            x: dx - ratio * (dx - self.x),
            y: dy - ratio * (dy - self.y),
        }
        .clamp(viewport, image)
    }
}

fn fitted_size(natural: Size<Pixels>, viewport: Size<Pixels>) -> Size<Pixels> {
    let ratio = ((viewport.width.as_f32() - 48.).max(0.) / natural.width.as_f32().max(1.))
        .min((viewport.height.as_f32() - 48.).max(0.) / natural.height.as_f32().max(1.))
        .min(1.);
    size(natural.width * ratio, natural.height * ratio)
}

struct EditableText {
    position: (f32, f32),
    size: f32,
    color: u32,
    input: Entity<crate::input::Input>,
    _change: gpui::Subscription,
}

pub(crate) struct Closed;
pub(crate) struct ImageLightbox {
    source: Source,
    alt: SharedString,
    image: Option<Arc<RenderImage>>,
    failed: bool,
    focus: FocusHandle,
    restore: Option<FocusHandle>,
    viewport: Size<Pixels>,
    image_size: Size<Pixels>,
    view: View,
    transition: Option<(View, Instant)>,
    drag: Option<(View, Point<Pixels>)>,
    owner: Option<DraftOwner>,
    generation: Option<u64>,
    original: Option<Arc<image::RgbaImage>>,
    strokes: Vec<annotation::Stroke>,
    drawing: bool,
    brush: bool,
    text_tool: bool,
    texts: Vec<EditableText>,
    close_after_save: bool,
    color: u32,
    palette_open: bool,
    picker: Entity<crate::color_picker::ColorPicker>,
    _picker_subscription: gpui::Subscription,
    saving: bool,
    cancelled: bool,
    error: Option<String>,
}
impl EventEmitter<Closed> for ImageLightbox {}

/// The draft an annotated image returns to. Conversations implement this, so the
/// lightbox has no dependency on them.
pub(crate) trait AnnotationDraft {
    /// The draft's generation, once its current edits are saved.
    fn begin(&self, cx: &mut App) -> Option<u64>;
    /// Replaces the source while `generation` still owns it.
    fn apply(
        &self,
        source: &std::path::Path,
        generation: u64,
        bytes: &[u8],
        cx: &mut App,
    ) -> Result<(PathBuf, u64), String>;
}
pub(crate) type DraftOwner = std::rc::Rc<dyn AnnotationDraft>;

impl ImageLightbox {
    pub(crate) fn new(
        open: Open,
        owner: Option<DraftOwner>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let generation = owner.as_ref().and_then(|owner| owner.begin(cx));
        let restore = window.focused(cx);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let picker = cx.new(|cx| crate::color_picker::ColorPicker::new(0xff3b30, cx));
        let picker_subscription = cx.subscribe(
            &picker,
            |this, _, picked: &crate::color_picker::Picked, cx| {
                this.color = picked.0;
                cx.notify();
            },
        );
        if owner.is_some()
            && let Source::Path(path) = &open.source
        {
            let path = path.clone();
            let renderer = cx.svg_renderer();
            let work = cx.background_executor().spawn(async move {
                let pixels = annotation::read_image(&path)?;
                let bytes = annotation::png(&pixels, &[], &[])?;
                let image = Image::from_bytes(ImageFormat::Png, bytes)
                    .to_image_data(renderer)
                    .map_err(|e| e.to_string())?;
                Ok::<_, String>((Arc::new(pixels), image))
            });
            cx.spawn(async move |this, cx| {
                let result = work.await;
                let _ = this.update(cx, |this, cx| {
                    match result {
                        Ok((pixels, image)) => {
                            this.original = Some(pixels);
                            this.image = Some(image);
                        }
                        Err(error) => {
                            this.owner = None;
                            this.error = Some(format!("Annotation unavailable: {error}"));
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
        } else if let Source::Embedded { data, mime } = &open.source {
            let data = data.clone();
            let format = ImageFormat::from_mime_type(mime).unwrap_or(ImageFormat::Png);
            let renderer = cx.svg_renderer();
            let work = cx.background_executor().spawn(async move {
                let bytes = vorkspace_core::composer::decode_image_data(&data).ok()?;
                Image::from_bytes(format, bytes)
                    .to_image_data(renderer)
                    .ok()
            });
            cx.spawn(async move |this, cx| {
                let image = work.await;
                let _ = this.update(cx, |this, cx| {
                    this.failed = image.is_none();
                    this.image = image;
                    cx.notify();
                });
            })
            .detach();
        }
        Self {
            source: open.source,
            alt: open.alt,
            image: None,
            failed: false,
            focus,
            restore,
            viewport: window.viewport_size(),
            image_size: size(px(0.), px(0.)),
            view: View::IDENTITY,
            transition: None,
            drag: None,
            owner,
            generation,
            original: None,
            strokes: Vec::new(),
            drawing: false,
            brush: false,
            text_tool: false,
            texts: Vec::new(),
            close_after_save: false,
            color: 0xff3b30,
            palette_open: false,
            picker,
            _picker_subscription: picker_subscription,
            saving: false,
            cancelled: false,
            error: None,
        }
    }

    fn image_point(&self, position: Point<Pixels>, clamp: bool) -> Option<(f32, f32)> {
        let bounds = self.image_bounds(self.displayed_view());
        if bounds.size.width <= px(0.)
            || bounds.size.height <= px(0.)
            || (!clamp && !bounds.contains(&position))
        {
            return None;
        }
        Some((
            ((position.x - bounds.left()) / bounds.size.width).clamp(0., 1.),
            ((position.y - bounds.top()) / bounds.size.height).clamp(0., 1.),
        ))
    }

    fn has_edits(&self, cx: &App) -> bool {
        !self.strokes.is_empty()
            || self
                .texts
                .iter()
                .any(|text| !text.input.read(cx).content.is_empty())
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || !self.has_edits(cx) {
            return;
        }
        let (Some(owner), Some(original), Source::Path(path)) =
            (self.owner.clone(), self.original.clone(), &self.source)
        else {
            return;
        };
        let path = path.clone();
        let Some(generation) = self.generation else {
            return;
        };
        let strokes = self.strokes.clone();
        let texts: Vec<_> = self
            .texts
            .iter()
            .map(|text| annotation::TextBox {
                position: text.position,
                size: text.size,
                color: text.color,
                text: text.input.read(cx).content.to_string(),
            })
            .collect();
        for text in &self.texts {
            text.input.update(cx, |input, _| input.set_read_only(true));
        }
        self.drawing = false;
        self.saving = true;
        self.error = None;
        let renderer = cx.svg_renderer();
        let work = cx.background_executor().spawn(async move {
            let bytes = annotation::png(&original, &strokes, &texts)?;
            let image = Image::from_bytes(ImageFormat::Png, bytes.clone())
                .to_image_data(renderer)
                .map_err(|e| e.to_string())?;
            let pixels = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
                .map_err(|e| e.to_string())?
                .into_rgba8();
            Ok::<_, String>((bytes, image, pixels))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.cancelled {
                    return;
                }
                let result = result.and_then(|(bytes, image, pixels)| {
                    let (path, generation) = owner.apply(&path, generation, &bytes, cx)?;
                    Ok::<_, String>((path, image, pixels, generation))
                });
                this.saving = false;
                for text in &this.texts {
                    text.input.update(cx, |input, _| input.set_read_only(false));
                }
                match result {
                    Ok((path, image, pixels, generation)) => {
                        this.generation = Some(generation);
                        this.source = Source::Path(path);
                        this.image = Some(image);
                        this.original = Some(Arc::new(pixels));
                        this.strokes.clear();
                        let had_text = !this.texts.is_empty();
                        this.texts.clear();
                        this.error = None;
                        if had_text && !this.close_after_save {
                            window.focus(&this.focus, cx);
                        }
                        if this.close_after_save {
                            this.finish_close(window, cx);
                        }
                    }
                    Err(error) => {
                        this.close_after_save = false;
                        this.error = Some(format!("Could not save annotation: {error}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let button = |id: &'static str, label: &'static str| {
            crate::controls::icon_button(id, label, true)
                .size(px(36.))
                .rounded_full()
                .border_1()
                .border_color(white(0.15))
                .bg(black(0.45))
                .text_color(white(0.85))
                .hover(|s| s.bg(black(0.65)).text_color(white(1.)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        };
        div()
            .absolute()
            .top(px(16.))
            .right(px(60.))
            .flex()
            .gap(px(6.))
            .child(
                button("annotation-color", "Annotation color")
                    .debug_selector(|| "annotation-color".into())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.palette_open = !this.palette_open;
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .child(
                        div()
                            .size(px(17.))
                            .rounded_full()
                            .bg(gpui::rgb(self.color))
                            .border_1()
                            .border_color(white(0.7)),
                    ),
            )
            .child(
                button("annotation-brush", "Toggle brush")
                    .debug_selector(|| "annotation-brush".into())
                    .aria_toggled(if self.brush {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .when(self.brush, |d| d.bg(white(0.25)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.brush = !this.brush;
                        this.text_tool = false;
                        this.drawing = false;
                        this.palette_open = false;
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .child(icon("annotation-brush-icon", 16.)),
            )
            .child(
                button("annotation-text", "Add text")
                    .debug_selector(|| "annotation-text".into())
                    .aria_toggled(if self.text_tool {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .when(self.text_tool, |d| d.bg(white(0.25)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.text_tool = !this.text_tool;
                        this.brush = false;
                        this.drawing = false;
                        this.palette_open = false;
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .child("T"),
            )
            .child(
                button("annotation-clear", "Clear drawing")
                    .debug_selector(|| "annotation-clear".into())
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.saving {
                            return;
                        }
                        if !this.texts.is_empty() {
                            window.focus(&this.focus, cx);
                        }
                        this.strokes.clear();
                        this.texts.clear();
                        this.drawing = false;
                        this.error = None;
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .child(icon("trash", 16.)),
            )
            .child(
                button("annotation-save", "Save drawing to attachment (⌘S)")
                    .debug_selector(|| "annotation-save".into())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.save(window, cx);
                        cx.stop_propagation();
                    }))
                    .child(icon("annotation-save-icon", 16.)),
            )
            .when(self.palette_open, |d| {
                d.child(
                    div()
                        .id("annotation-color-popover")
                        .absolute()
                        .top(px(44.))
                        .left(px(-80.))
                        .w(px(220.))
                        .p(px(10.))
                        .rounded_lg()
                        .bg(gpui::rgb(if vorkspace_ui::theme::is_light() {
                            0xf4f4f4
                        } else {
                            0x262628
                        }))
                        .text_color(vorkspace_ui::theme::content(1.))
                        .border_1()
                        .border_color(white(0.15))
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(self.picker.clone()),
                )
            })
    }

    pub(crate) fn cancel(&mut self) {
        self.cancelled = true;
    }

    pub(crate) fn reject_replacement(&mut self, cx: &mut Context<Self>) {
        self.error = Some("Close this image preview before opening another".into());
        cx.notify();
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.saving {
            self.close_after_save = true;
        } else if self.has_edits(cx) {
            self.close_after_save = true;
            self.save(window, cx);
            if !self.saving {
                self.close_after_save = false;
                self.error = Some("Could not save annotation: the draft is unavailable".into());
                cx.notify();
            }
        } else {
            self.finish_close(window, cx);
        }
    }

    // Escape discards the unsaved overlay, including an in-flight encoding.
    fn finish_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel();
        if let Some(restore) = self.restore.take() {
            window.focus(&restore, cx);
        }
        cx.emit(Closed);
        cx.stop_propagation();
    }

    fn displayed_view(&self) -> View {
        let Some((from, born)) = self.transition else {
            return self.view;
        };
        let t = vorkspace_ui::reveal::ease_out((born.elapsed().as_secs_f32() / 0.06).min(1.));
        View {
            scale: from.scale + (self.view.scale - from.scale) * t,
            x: from.x + (self.view.x - from.x) * t,
            y: from.y + (self.view.y - from.y) * t,
        }
    }

    fn set_view(&mut self, next: View, cx: &mut Context<Self>) {
        let next = next.clamp(self.viewport, self.image_size);
        if next != self.view {
            self.transition = self
                .drag
                .is_none()
                .then(|| (self.displayed_view(), Instant::now()));
            self.view = next;
            cx.notify();
        }
    }

    fn zoom_at(&mut self, scale: f32, pointer: Point<Pixels>, cx: &mut Context<Self>) {
        self.set_view(
            self.view
                .zoom_at(scale, pointer, self.viewport, self.image_size),
            cx,
        );
    }

    fn zoom_key(&mut self, factor: f32, cx: &mut Context<Self>) {
        self.zoom_at(
            self.view.scale * factor,
            point(self.viewport.width / 2., self.viewport.height / 2.),
            cx,
        );
        cx.stop_propagation();
    }

    fn image_bounds(&self, view: View) -> Bounds<Pixels> {
        let dimensions = size(
            self.image_size.width * view.scale,
            self.image_size.height * view.scale,
        );
        Bounds::new(
            point(
                (self.viewport.width - dimensions.width) / 2. + px(view.x),
                (self.viewport.height - dimensions.height) / 2. + px(view.y),
            ),
            dimensions,
        )
    }
}

impl Render for ImageLightbox {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.image.is_none()
            && !self.failed
            && self.owner.is_none()
            && let Source::Path(path) = &self.source
            && let Some(result) = window.use_asset::<ImgResourceLoader>(&path.clone().into(), cx)
        {
            match result {
                Ok(image) => self.image = Some(image),
                Err(_) => self.failed = true,
            }
        }
        self.viewport = window.viewport_size();
        if let Some(image) = &self.image {
            let natural = image.size(0);
            self.image_size = fitted_size(
                size(px(natural.width.0 as f32), px(natural.height.0 as f32)),
                self.viewport,
            );
        }
        self.view = self.view.clamp(self.viewport, self.image_size);
        let view = self.displayed_view().clamp(self.viewport, self.image_size);
        if self
            .transition
            .is_some_and(|(_, born)| born.elapsed().as_secs_f32() < 0.06)
        {
            window.request_animation_frame();
        } else {
            self.transition = None;
        }
        let bounds = self.image_bounds(view);
        let zoomed = self.view.scale > MIN_SCALE;
        div()
            .id("image-lightbox")
            .debug_selector(|| "image-lightbox".into())
            .role(Role::Dialog)
            .aria_label(format!("Image preview: {}", self.alt))
            .key_context("ImageLightbox")
            .absolute()
            .top_0()
            .left_0()
            .w(self.viewport.width)
            .h(self.viewport.height)
            .overflow_hidden()
            .on_action(cx.listener(|this, _: &Close, window, cx| this.finish_close(window, cx)))
            .on_action(cx.listener(|this, _: &Save, window, cx| {
                this.save(window, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.zoom_key(KEY_ZOOM_STEP, cx)))
            .on_action(
                cx.listener(|this, _: &ZoomOut, _, cx| this.zoom_key(1. / KEY_ZOOM_STEP, cx)),
            )
            .on_action(cx.listener(|this, _: &ResetZoom, _, cx| {
                this.set_view(View::IDENTITY, cx);
                cx.stop_propagation();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    if !this
                        .image_bounds(this.displayed_view())
                        .contains(&event.position)
                        || this.image.is_none()
                    {
                        this.close(window, cx);
                    } else if this.text_tool && this.original.is_some() && !this.saving {
                        this.view = this.displayed_view();
                        this.transition = None;
                        if let Some(position) = this.image_point(event.position, false) {
                            let text_size = 20. / this.image_size.width.as_f32().max(1.);
                            let input = cx.new(|cx| {
                                crate::input::Input::field(
                                    "Annotation text",
                                    "Type text",
                                    white(0.6),
                                    cx,
                                )
                            });
                            let change = cx
                                .subscribe(&input, |_, _, _: &crate::input::Changed, cx| {
                                    cx.notify()
                                });
                            window.focus(&input.focus_handle(cx), cx);
                            this.texts.push(EditableText {
                                position,
                                size: text_size,
                                color: this.color,
                                input,
                                _change: change,
                            });
                            cx.notify();
                        }
                    } else if this.brush && this.original.is_some() && !this.saving {
                        this.view = this.displayed_view();
                        this.transition = None;
                        if let Some(p) = this.image_point(event.position, false) {
                            this.strokes.push(annotation::Stroke {
                                color: this.color,
                                width: 4. / this.image_size.width.as_f32().max(1.),
                                points: vec![p],
                            });
                            this.drawing = true;
                            cx.notify();
                        }
                    } else if event.click_count == 2 {
                        this.drag = None;
                        if this.view.scale > MIN_SCALE {
                            this.set_view(View::IDENTITY, cx);
                        } else {
                            this.zoom_at(DOUBLE_CLICK_SCALE, event.position, cx);
                        }
                    } else if this.view.scale > MIN_SCALE {
                        this.transition = None;
                        this.drag = Some((this.view, event.position));
                        cx.notify();
                    }
                }),
            )
            .on_click(|_, _, cx| cx.stop_propagation())
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                if this.drawing {
                    if event.pressed_button == Some(MouseButton::Left) {
                        if let Some(p) = this.image_point(event.position, true)
                            && let Some(stroke) = this.strokes.last_mut()
                            && stroke.points.last() != Some(&p)
                        {
                            stroke.points.push(p);
                        }
                        if !this
                            .image_bounds(this.displayed_view())
                            .contains(&event.position)
                        {
                            this.drawing = false;
                        }
                    } else {
                        this.drawing = false;
                    }
                    cx.notify();
                } else if let Some((origin, pointer)) = this.drag {
                    if event.pressed_button == Some(MouseButton::Left) {
                        this.set_view(
                            View {
                                x: origin.x + (event.position.x - pointer.x).as_f32(),
                                y: origin.y + (event.position.y - pointer.y).as_f32(),
                                ..origin
                            },
                            cx,
                        );
                    } else {
                        this.drag = None;
                        cx.notify();
                    }
                }
                cx.stop_propagation();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseUpEvent, _, cx| {
                    if this.drawing
                        && let Some(p) = this.image_point(event.position, true)
                        && let Some(stroke) = this.strokes.last_mut()
                        && stroke.points.last() != Some(&p)
                    {
                        stroke.points.push(p);
                    }
                    this.drag = None;
                    this.drawing = false;
                    cx.notify();
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.drag = None;
                    this.drawing = false;
                    cx.notify();
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                let delta = event.delta.pixel_delta(px(16.));
                if event.modifiers.control || event.modifiers.platform {
                    // GPUI's wheel deltas have the opposite sign to DOM wheel deltas.
                    this.zoom_at(
                        this.view.scale * (delta.y.as_f32() * 0.01).exp(),
                        event.position,
                        cx,
                    );
                } else if this.view.scale > MIN_SCALE {
                    this.set_view(
                        View {
                            x: this.view.x + delta.x.as_f32(),
                            y: this.view.y + delta.y.as_f32(),
                            ..this.view
                        },
                        cx,
                    );
                }
                cx.stop_propagation();
            }))
            .on_pinch(cx.listener(|this, event: &gpui::PinchEvent, _, cx| {
                this.zoom_at(this.view.scale * (1. + event.delta), event.position, cx);
                cx.stop_propagation();
            }))
            .child(
                canvas(
                    |_, _, _| {},
                    |bounds, _, window, _| {
                        let quad = gpui::fill(bounds, black(0.85));
                        let quad =
                            quad.border_color(gpui_apple::metal_renderer::backdrop_blur_marker(
                                4. * window.scale_factor(),
                            ));
                        window.paint_quad(quad);
                    },
                )
                .absolute()
                .size_full(),
            )
            .when_some(self.image.clone(), |d, image| {
                d.child(
                    div()
                        .absolute()
                        .left(bounds.left())
                        .top(bounds.top())
                        .w(bounds.size.width)
                        .h(bounds.size.height)
                        .debug_selector(|| "lightbox-image".into())
                        .cursor(if self.brush || self.text_tool {
                            CursorStyle::Crosshair
                        } else if !zoomed {
                            CursorStyle::PointingHand
                        } else if self.drag.is_some() {
                            CursorStyle::ClosedHand
                        } else {
                            CursorStyle::OpenHand
                        })
                        .shadow(vec![
                            BoxShadow::new(px(0.), px(25.), black(0.25))
                                .blur_radius(px(50.))
                                .spread_radius(px(-12.)),
                        ])
                        .overflow_hidden()
                        .child(img(image).size_full())
                        .child({
                            let strokes = self.strokes.clone();
                            canvas(
                                |_, _, _| {},
                                move |bounds, _, window, _| {
                                    for stroke in &strokes {
                                        let to_screen = |p: (f32, f32)| {
                                            point(
                                                bounds.left() + bounds.size.width * p.0,
                                                bounds.top() + bounds.size.height * p.1,
                                            )
                                        };
                                        let width = bounds.size.width * stroke.width;
                                        for pair in stroke.points.windows(2) {
                                            let mut path = gpui::PathBuilder::stroke(width);
                                            path.move_to(to_screen(pair[0]));
                                            path.line_to(to_screen(pair[1]));
                                            if let Ok(path) = path.build() {
                                                window.paint_path(path, gpui::rgb(stroke.color));
                                            }
                                        }
                                        for p in &stroke.points {
                                            let p = to_screen(*p);
                                            window.paint_quad(
                                                gpui::fill(
                                                    Bounds::new(
                                                        point(p.x - width / 2., p.y - width / 2.),
                                                        size(width, width),
                                                    ),
                                                    gpui::rgb(stroke.color),
                                                )
                                                .corner_radii(width / 2.),
                                            );
                                        }
                                    }
                                },
                            )
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                        })
                        .children(self.texts.iter().enumerate().map(|(index, text)| {
                            let font_size = (bounds.size.width.as_f32() * text.size).max(1.);
                            text.input
                                .update(cx, |input, _| input.set_annotation_metrics(font_size));
                            div()
                                .id(("annotation-text-box", index))
                                .debug_selector(move || format!("annotation-text-box-{index}"))
                                .absolute()
                                .left(bounds.size.width * text.position.0)
                                .top(bounds.size.height * text.position.1)
                                .w(bounds.size.width * (1. - text.position.0))
                                .text_color(gpui::rgb(text.color))
                                .font(gpui::font(annotation::FONT_FAMILY))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(text.input.clone())
                        })),
                )
            })
            .when(self.failed, |d| {
                d.child(
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(white(0.8))
                        .child("Couldn’t load image"),
                )
            })
            .when(self.saving, |d| {
                d.child(
                    div()
                        .absolute()
                        .bottom(px(20.))
                        .left(px(24.))
                        .text_color(white(0.8))
                        .child("Saving drawing…"),
                )
            })
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    div()
                        .debug_selector(|| "annotation-error".into())
                        .absolute()
                        .bottom(px(20.))
                        .left(px(24.))
                        .text_color(gpui::rgb(0xffb4ac))
                        .child(error),
                )
            })
            .when(self.owner.is_some() && self.original.is_some(), |d| {
                d.child(self.toolbar(cx)).child(
                    div()
                        .absolute()
                        .bottom(px(20.))
                        .right(px(24.))
                        .text_size(px(12.))
                        .text_color(white(0.7))
                        .child("Close saves · Esc discards unsaved edits"),
                )
            })
            .child(
                div()
                    .id("close-image-preview")
                    .group("image-preview-close")
                    .debug_selector(|| "close-image-preview".into())
                    .role(Role::Button)
                    .aria_label("Close image preview")
                    .track_focus(&self.focus)
                    .absolute()
                    .top(px(16.))
                    .right(px(16.))
                    .size(px(36.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .border_1()
                    .border_color(white(0.15))
                    .bg(black(0.45))
                    .text_color(white(0.8))
                    .shadow_lg()
                    .cursor(CursorStyle::Arrow)
                    .hover(|s| s.bg(black(0.65)).text_color(white(1.)))
                    .focus_visible(|s| s.border_2().border_color(white(0.8)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, window, cx| this.close(window, cx)))
                    .child(
                        canvas(
                            |_, _, _| {},
                            |bounds, _, window, _| {
                                window.paint_quad(
                                    gpui::fill(bounds, gpui::transparent_black())
                                        .corner_radii(px(18.))
                                        .border_color(
                                            gpui_apple::metal_renderer::backdrop_blur_marker(
                                                12. * window.scale_factor(),
                                            ),
                                        ),
                                );
                            },
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(
                        icon("close", 16.)
                            .text_color(gpui::rgb(0xff453a))
                            .group_hover("image-preview-close", |s| {
                                s.text_color(gpui::rgb(0xff6961))
                            }),
                    ),
            )
    }
}

pub(crate) fn overlay(view: &Entity<ImageLightbox>) -> impl IntoElement {
    gpui::deferred(
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(view.clone()),
    )
    .with_priority(100)
}

#[cfg(test)]
mod tests;
