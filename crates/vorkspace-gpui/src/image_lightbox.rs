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
    owner: Option<gpui::WeakEntity<crate::conversation::Conversation>>,
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
    picker: Entity<crate::bots::color_picker::ColorPicker>,
    _picker_subscription: gpui::Subscription,
    saving: bool,
    cancelled: bool,
    error: Option<String>,
}
impl EventEmitter<Closed> for ImageLightbox {}

impl ImageLightbox {
    pub(crate) fn new(
        open: Open,
        owner: Option<Entity<crate::conversation::Conversation>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let generation = owner
            .as_ref()
            .map(|owner| owner.update(cx, |owner, cx| owner.annotation_draft_generation(cx)));
        let restore = window.focused(cx);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let picker = cx.new(|cx| crate::bots::color_picker::ColorPicker::new(0xff3b30, cx));
        let picker_subscription = cx.subscribe(
            &picker,
            |this, _, picked: &crate::bots::color_picker::Picked, cx| {
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
            owner: owner.map(|owner| owner.downgrade()),
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
                    let owner = owner.upgrade().ok_or("The draft was closed")?;
                    let path = owner.update(cx, |owner, cx| {
                        owner.apply_annotated_attachment(&path, generation, &bytes, cx)
                    })?;
                    let generation = owner.read(cx).attachment_draft_generation();
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
mod tests {
    use super::*;
    use gpui::{
        Modifiers, ScrollDelta, ScrollWheelEvent, TestAppContext, TouchPhase, VisualTestContext,
        WindowBounds, WindowOptions,
    };

    #[test]
    #[ignore = "Requires Metal and VORKSPACE_ANNOTATION_CAPTURE_DIR"]
    fn native_annotation_capture() {
        let output =
            std::path::PathBuf::from(std::env::var_os("VORKSPACE_ANNOTATION_CAPTURE_DIR").unwrap());
        assert!(output.is_absolute() && output.is_dir());
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sample.png");
        let mut sample = image::RgbaImage::from_pixel(800, 480, image::Rgba([235, 237, 240, 255]));
        for y in 85..395 {
            for x in 110..690 {
                let pixel = if y < 145 {
                    [41, 47, 57, 255]
                } else {
                    [252, 252, 252, 255]
                };
                sample.put_pixel(x, y, image::Rgba(pixel));
            }
        }
        sample.save(&path).unwrap();
        let mut cx = gpui::HeadlessAppContext::with_platform(
            Arc::new(gpui_apple::AppleTextSystem::new()),
            Arc::new(crate::assets::Assets),
            || {
                Ok(Some(Box::new(
                    gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
                )))
            },
        );
        let store = vorkspace_core::store::Store::open(std::path::Path::new(":memory:")).unwrap();
        let session = store.create("codex", "/missing/capture").unwrap();
        for (width, height) in [(1000., 700.), (560., 450.)] {
            let owner = cx.update(|cx| {
                cx.new(|cx| {
                    crate::conversation::Conversation::new(
                        session.clone(),
                        Arc::new(std::sync::Mutex::new(
                            vorkspace_core::store::Store::open(std::path::Path::new(":memory:"))
                                .unwrap(),
                        )),
                        cx,
                    )
                })
            });
            let window = cx
                .open_window(size(px(width), px(height)), |window, cx| {
                    cx.new(|cx| {
                        ImageLightbox::new(
                            Open {
                                source: Source::Path(path.clone()),
                                alt: "Fixture".into(),
                                draft_session: Some(session.id),
                            },
                            Some(owner),
                            window,
                            cx,
                        )
                    })
                })
                .unwrap();
            cx.run_until_parked();
            window
                .update(&mut cx, |view, _, cx| {
                    assert!(view.original.is_some());
                    view.brush = true;
                    let input = cx.new(|cx| {
                        let mut input = crate::input::Input::field(
                            "Annotation text",
                            "Type text",
                            white(0.6),
                            cx,
                        );
                        input.set_text("Review this".into(), cx);
                        input
                    });
                    let change =
                        cx.subscribe(&input, |_, _, _: &crate::input::Changed, cx| cx.notify());
                    view.texts.push(EditableText {
                        position: (0.25, 0.65),
                        size: 0.035,
                        color: 0x0055ff,
                        input,
                        _change: change,
                    });
                    view.strokes.push(annotation::Stroke {
                        color: 0xff3b30,
                        width: 0.01,
                        points: vec![
                            (0.32, 0.45),
                            (0.45, 0.34),
                            (0.63, 0.47),
                            (0.45, 0.58),
                            (0.32, 0.45),
                        ],
                    });
                    cx.notify();
                })
                .unwrap();
            cx.run_until_parked();
            let bounds = window
                .update(&mut cx, |view, _, _| view.image_bounds(view.view))
                .unwrap();
            let capture = cx.capture_screenshot(window.into()).unwrap();
            let scale_x = capture.width() as f32 / width;
            let scale_y = capture.height() as f32 / height;
            let x = ((bounds.left() + bounds.size.width * 0.45).as_f32() * scale_x) as u32;
            let y = ((bounds.top() + bounds.size.height * 0.34).as_f32() * scale_y) as u32;
            let pixel = capture.get_pixel(x, y);
            assert!(
                pixel[0] > 200 && pixel[1] < 100 && pixel[2] < 100,
                "stroke was not rendered at the image coordinate: {pixel:?}"
            );
            capture
                .save(output.join(format!("annotation-{width}.png")))
                .unwrap();
            window
                .update(&mut cx, |view, _, cx| {
                    view.palette_open = true;
                    cx.notify();
                })
                .unwrap();
            cx.run_until_parked();
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(output.join(format!("annotation-picker-{width}.png")))
                .unwrap();
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
        }
    }

    #[test]
    #[ignore = "Requires Metal"]
    fn annotation_preview_matches_flattened_text_at_zoom() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("text.png");
        let original = image::RgbaImage::from_pixel(320, 120, image::Rgba([255, 255, 255, 255]));
        original.save(&path).unwrap();
        let mut cx = gpui::HeadlessAppContext::with_platform(
            Arc::new(gpui_apple::AppleTextSystem::new()),
            Arc::new(crate::assets::Assets),
            || {
                Ok(Some(Box::new(
                    gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
                )))
            },
        );
        let store = vorkspace_core::store::Store::open(std::path::Path::new(":memory:")).unwrap();
        let session = store.create("codex", "/missing/text-preview").unwrap();
        let owner = cx.update(|cx| {
            cx.new(|cx| {
                crate::conversation::Conversation::new(
                    session.clone(),
                    Arc::new(std::sync::Mutex::new(
                        vorkspace_core::store::Store::open(std::path::Path::new(":memory:"))
                            .unwrap(),
                    )),
                    cx,
                )
            })
        });
        let window = cx
            .open_window(size(px(800.), px(600.)), |window, cx| {
                cx.new(|cx| {
                    ImageLightbox::new(
                        Open {
                            source: Source::Path(path.clone()),
                            alt: "Text fixture".into(),
                            draft_session: Some(session.id),
                        },
                        Some(owner),
                        window,
                        cx,
                    )
                })
            })
            .unwrap();
        cx.run_until_parked();
        let texts = [
            annotation::TextBox {
                position: (0.08, 0.18),
                size: 0.07,
                color: 0x000000,
                text: "Wi fi 漢字".into(),
            },
            annotation::TextBox {
                position: (0.82, 0.64),
                size: 0.07,
                color: 0x000000,
                text: "Wi".into(),
            },
        ];
        let flattened = image::load_from_memory(&annotation::png(&original, &[], &texts).unwrap())
            .unwrap()
            .into_rgba8();
        window
            .update(&mut cx, |view, _, cx| {
                assert!(view.original.is_some());
                for text in &texts {
                    let input = cx.new(|cx| {
                        let mut input = crate::input::Input::field(
                            "Annotation text",
                            "Type text",
                            white(0.6),
                            cx,
                        );
                        input.set_text(text.text.clone(), cx);
                        input
                    });
                    let change =
                        cx.subscribe(&input, |_, _, _: &crate::input::Changed, cx| cx.notify());
                    view.texts.push(EditableText {
                        position: text.position,
                        size: text.size,
                        color: text.color,
                        input,
                        _change: change,
                    });
                }
                cx.notify();
            })
            .unwrap();
        for zoom in [1., 2.] {
            window
                .update(&mut cx, |view, _, cx| {
                    view.view = View {
                        scale: zoom,
                        x: 0.,
                        y: 0.,
                    };
                    view.transition = None;
                    cx.notify();
                })
                .unwrap();
            cx.run_until_parked();
            let bounds = window
                .update(&mut cx, |view, _, _| view.image_bounds(view.view))
                .unwrap();
            let capture = cx.capture_screenshot(window.into()).unwrap();
            let sx = capture.width() as f32 / 800.;
            let sy = capture.height() as f32 / 600.;
            // Compare the actual editor pixels with the PNG, allowing a small
            // neighborhood for device-scale rasterization and antialiasing.
            let mut preview = vec![false; (320 * 120) as usize];
            let mut saved = preview.clone();
            for y in 0..120u32 {
                for x in 0..320u32 {
                    let index = (y * 320 + x) as usize;
                    saved[index] = flattened.get_pixel(x, y)[0] < 220;
                    let screen_x = ((bounds.left().as_f32() + (x as f32 + 0.5) * zoom) * sx) as u32;
                    let screen_y = ((bounds.top().as_f32() + (y as f32 + 0.5) * zoom) * sy) as u32;
                    preview[index] = capture.get_pixel(screen_x, screen_y)[0] < 220;
                }
            }
            let overlap = |from: &[bool], to: &[bool]| {
                let mut count = 0;
                let mut matched = 0;
                for y in 0..120i32 {
                    for x in 0..320i32 {
                        if !from[(y * 320 + x) as usize] {
                            continue;
                        }
                        count += 1;
                        if (-1..=1).any(|dy| {
                            (-1..=1).any(|dx| {
                                let (x, y) = (x + dx, y + dy);
                                (0..320).contains(&x)
                                    && (0..120).contains(&y)
                                    && to[(y * 320 + x) as usize]
                            })
                        }) {
                            matched += 1;
                        }
                    }
                }
                assert!(count > 30, "text must be visible at zoom {zoom}");
                assert!(
                    matched * 100 >= count * 85,
                    "text differs at zoom {zoom}: {matched}/{count}"
                );
            };
            overlap(&preview, &saved);
            overlap(&saved, &preview);
        }
        // A rejected edge placement must leave both the source file and the
        // user's editable input intact, even after the asynchronous save.
        let source_before = std::fs::read(&path).unwrap();
        window
            .update(&mut cx, |view, window, cx| {
                view.texts[0].position = (0.99, 0.18);
                view.save(window, cx);
                assert!(view.saving);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(&mut cx, |view, _, cx| {
                assert!(!view.saving);
                assert!(view.error.as_deref().unwrap().contains("beyond the image"));
                assert_eq!(view.texts[0].input.read(cx).content.as_ref(), "Wi fi 漢字");
                assert_eq!(view.texts[0].position, (0.99, 0.18));
                assert_eq!(view.source, Source::Path(path.clone()));
                assert_eq!(view.original.as_ref().unwrap().as_ref(), &original);
            })
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), source_before);
    }

    #[gpui::test]
    fn drawing_tracks_image_coordinates_and_clear_across_zoom_and_resize(cx: &mut TestAppContext) {
        cx.update(crate::input::bind);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sample.png");
        image::RgbaImage::from_pixel(800, 400, image::Rgba([255, 255, 255, 255]))
            .save(&path)
            .unwrap();
        let store = vorkspace_core::store::Store::open(std::path::Path::new(":memory:")).unwrap();
        let session = store.create("codex", "/missing/test").unwrap();
        let owner = cx.new(|cx| {
            crate::conversation::Conversation::new(
                session,
                Arc::new(std::sync::Mutex::new(store)),
                cx,
            )
        });
        let window = cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(800.), px(600.)),
                    ))),
                    ..Default::default()
                },
                |window, cx| {
                    cx.new(|cx| {
                        ImageLightbox::new(
                            Open {
                                source: Source::Path(path),
                                alt: "Sample".into(),
                                draft_session: Some(1),
                            },
                            Some(owner),
                            window,
                            cx,
                        )
                    })
                },
            )
            .unwrap()
        });
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.refresh().unwrap();
        window
            .update(visual, |view, _, _| {
                assert_eq!(view.color, 0xff3b30);
                assert!(view.original.is_some());
            })
            .unwrap();
        let brush = visual.debug_bounds("annotation-brush").unwrap().center();
        visual.simulate_click(brush, Default::default());
        let bounds = visual.debug_bounds("lightbox-image").unwrap();
        let start = point(
            bounds.left() + bounds.size.width * 0.25,
            bounds.top() + bounds.size.height * 0.25,
        );
        let end = point(
            bounds.left() + bounds.size.width * 0.75,
            bounds.top() + bounds.size.height * 0.75,
        );
        visual.simulate_mouse_down(start, MouseButton::Left, Default::default());
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Default::default());
        visual.simulate_mouse_up(end, MouseButton::Left, Default::default());
        window
            .update(visual, |view, _, _| {
                assert_eq!(view.strokes.len(), 1);
                assert_eq!(view.strokes[0].color, 0xff3b30);
                assert_eq!(view.strokes[0].points, vec![(0.25, 0.25), (0.75, 0.75)]);
                view.view = View {
                    scale: 2.,
                    x: -40.,
                    y: 20.,
                };
                view.transition = None;
            })
            .unwrap();
        visual.simulate_resize(size(px(1000.), px(700.)));
        visual.refresh().unwrap();
        let bounds = visual.debug_bounds("lightbox-image").unwrap();
        let mapped = point(
            bounds.left() + bounds.size.width * 0.6,
            bounds.top() + bounds.size.height * 0.4,
        );
        window
            .update(visual, |view, _, _| {
                let p = view.image_point(mapped, false).unwrap();
                assert!((p.0 - 0.6).abs() < 0.0001 && (p.1 - 0.4).abs() < 0.0001);
                assert_eq!(view.strokes[0].points[0], (0.25, 0.25));
            })
            .unwrap();
        window
            .update(visual, |view, _, cx| {
                view.picker
                    .update(cx, |picker, cx| picker.set_color(0x0055ff, cx));
            })
            .unwrap();
        visual.run_until_parked();
        window
            .update(visual, |view, _, _| assert_eq!(view.color, 0x0055ff))
            .unwrap();
        let text_tool = visual.debug_bounds("annotation-text").unwrap().center();
        visual.simulate_click(text_tool, Default::default());
        let image_bounds = visual.debug_bounds("lightbox-image").unwrap();
        let position = point(
            image_bounds.left() + image_bounds.size.width * 0.5,
            image_bounds.top() + image_bounds.size.height * 0.5,
        );
        visual.simulate_click(position, Default::default());
        visual.simulate_input("Note");
        visual.refresh().unwrap();
        window
            .update(visual, |view, _, cx| {
                assert_eq!(view.texts.len(), 1);
                assert_eq!(view.texts[0].input.read(cx).content.as_ref(), "Note");
                assert_eq!(view.texts[0].color, 0x0055ff);
                assert!((view.texts[0].position.0 - 0.5).abs() < 0.001);
                assert!((view.texts[0].position.1 - 0.5).abs() < 0.001);
            })
            .unwrap();
        let before = visual.debug_bounds("annotation-text-box-0").unwrap();
        assert!((before.left() - position.x).as_f32().abs() < 1.);
        visual.simulate_resize(size(px(900.), px(650.)));
        visual.refresh().unwrap();
        let image_bounds = visual.debug_bounds("lightbox-image").unwrap();
        let after = visual.debug_bounds("annotation-text-box-0").unwrap();
        assert!(
            (after.left() - (image_bounds.left() + image_bounds.size.width * 0.5))
                .as_f32()
                .abs()
                < 1.
        );
        assert!(
            (after.top() - (image_bounds.top() + image_bounds.size.height * 0.5))
                .as_f32()
                .abs()
                < 1.
        );
        let clear = visual.debug_bounds("annotation-clear").unwrap().center();
        visual.simulate_click(clear, Default::default());
        window
            .update(visual, |view, _, _| {
                assert!(view.strokes.is_empty());
                assert!(view.texts.is_empty());
            })
            .unwrap();
        // Cancel in the same event turn in which encoding starts, before the
        // background executor can publish. The preview is deliberately kept
        // alive to prove cancellation does not rely on entity destruction.
        window
            .update(visual, |view, window, cx| {
                view.strokes.push(annotation::Stroke {
                    color: view.color,
                    width: 0.01,
                    points: vec![(0.5, 0.5)],
                });
                view.save(window, cx);
                assert!(view.saving);
                view.cancel();
            })
            .unwrap();
        visual.run_until_parked();
        window
            .update(visual, |view, _, _| {
                assert!(view.cancelled);
                assert!(view.saving);
                assert!(view.error.is_none());
                assert_eq!(view.strokes.len(), 1);
            })
            .unwrap();
    }

    #[test]
    fn fit_uses_reacts_24px_padding_without_upscaling_small_images() {
        let viewport = size(px(800.), px(600.));
        assert_eq!(
            fitted_size(size(px(100.), px(50.)), viewport),
            size(px(100.), px(50.))
        );
        assert_eq!(
            fitted_size(size(px(1600.), px(1200.)), viewport),
            size(px(736.), px(552.))
        );
    }

    #[test]
    fn zoom_keeps_the_pointer_pixel_fixed_and_clamps_pan_and_scale() {
        let viewport = size(px(800.), px(600.));
        let image = size(px(752.), px(552.));
        let pointer = point(px(500.), px(350.));
        let zoom = View::IDENTITY.zoom_at(2.5, pointer, viewport, image);
        assert_eq!(
            zoom,
            View {
                scale: 2.5,
                x: -150.,
                y: -75.
            }
        );
        assert_eq!((100. - zoom.x) / zoom.scale, 100.);
        assert_eq!((50. - zoom.y) / zoom.scale, 50.);
        assert_eq!(zoom.zoom_at(0.1, pointer, viewport, image), View::IDENTITY);
        assert_eq!(zoom.zoom_at(100., pointer, viewport, image).scale, 8.);
        assert_eq!(
            View {
                scale: 2.,
                x: 10000.,
                y: -10000.
            }
            .clamp(viewport, image),
            View {
                scale: 2.,
                x: 352.,
                y: -252.
            }
        );
        assert_eq!(
            View {
                scale: 8.,
                x: 100.,
                y: -100.
            }
            .clamp(viewport, size(px(20.), px(10.))),
            View {
                scale: 8.,
                x: 0.,
                y: 0.
            }
        );
    }

    #[gpui::test]
    fn lightbox_handles_keyboard_pinch_wheel_drag_double_click_and_resize(cx: &mut TestAppContext) {
        cx.update(bind);
        let window = cx.update(|cx| cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(0.), px(0.)), size(px(800.), px(600.)),
            ))),
            ..Default::default()
        }, |window, cx| cx.new(|cx| {
            let mut preview = ImageLightbox::new(Open { source: Source::Path("unused.png".into()), alt: "Test image".into(), draft_session: None }, None, window, cx);
            preview.image = Some(Image::from_bytes(ImageFormat::Svg, br##"<svg xmlns="http://www.w3.org/2000/svg" width="1600" height="1200"><rect width="1600" height="1200" fill="#4da3f5"/></svg>"##.to_vec()).to_image_data(cx.svg_renderer()).unwrap());
            preview
        })).unwrap());
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.refresh().unwrap();
        let modifier = "cmd";
        visual.simulate_keystrokes(&format!("{modifier}-="));
        window
            .update(visual, |view, _, _| assert_eq!(view.view.scale, 1.25))
            .unwrap();
        visual.simulate_keystrokes(&format!("{modifier}--"));
        window
            .update(visual, |view, _, _| assert_eq!(view.view, View::IDENTITY))
            .unwrap();
        let pointer = point(px(500.), px(350.));
        visual.simulate_mouse_move(pointer, None, Default::default());
        visual.simulate_event(gpui::PinchEvent {
            position: pointer,
            delta: 1.,
            modifiers: Default::default(),
            phase: TouchPhase::Moved,
        });
        window
            .update(visual, |view, _, _| {
                assert_eq!(
                    view.view,
                    View {
                        scale: 2.,
                        x: -100.,
                        y: -50.
                    }
                )
            })
            .unwrap();
        visual.simulate_event(ScrollWheelEvent {
            position: pointer,
            delta: ScrollDelta::Pixels(point(px(20.), px(-30.))),
            modifiers: Default::default(),
            touch_phase: TouchPhase::Moved,
        });
        window
            .update(visual, |view, _, _| {
                assert_eq!(
                    view.view,
                    View {
                        scale: 2.,
                        x: -80.,
                        y: -80.
                    }
                )
            })
            .unwrap();
        visual.simulate_event(ScrollWheelEvent {
            position: pointer,
            delta: ScrollDelta::Lines(point(0., 1.)),
            modifiers: Modifiers {
                control: true,
                ..Default::default()
            },
            touch_phase: TouchPhase::Moved,
        });
        window
            .update(visual, |view, _, _| {
                assert!((view.view.scale - 2. * 0.16_f32.exp()).abs() < 0.0001)
            })
            .unwrap();
        visual.simulate_keystrokes(&format!("{modifier}-0"));
        // Double-click a real image hitbox, then drag without a CSS transition.
        visual.simulate_event(gpui::MouseDownEvent {
            button: MouseButton::Left,
            position: pointer,
            click_count: 2,
            modifiers: Default::default(),
            first_mouse: false,
        });
        visual.simulate_mouse_up(pointer, MouseButton::Left, Default::default());
        window
            .update(visual, |view, _, _| {
                assert_eq!(view.view.scale, 2.5);
                view.transition = None;
            })
            .unwrap();
        visual.simulate_mouse_down(pointer, MouseButton::Left, Default::default());
        visual.simulate_mouse_move(
            point(px(520.), px(360.)),
            Some(MouseButton::Left),
            Default::default(),
        );
        window
            .update(visual, |view, _, _| {
                assert_eq!(
                    view.view,
                    View {
                        scale: 2.5,
                        x: -130.,
                        y: -65.
                    }
                );
                assert!(view.transition.is_none());
            })
            .unwrap();
        visual.simulate_mouse_up(pointer, MouseButton::Left, Default::default());
        visual.simulate_resize(size(px(1000.), px(700.)));
        visual.refresh().unwrap();
        assert_eq!(
            visual.debug_bounds("image-lightbox").unwrap().size,
            size(px(1000.), px(700.))
        );
        visual.simulate_keystrokes(&format!("{modifier}-0"));
        window
            .update(visual, |view, _, _| assert_eq!(view.view, View::IDENTITY))
            .unwrap();
        visual.update(|_, cx| {
            let mut prefs = crate::settings::Preferences::default();
            prefs
                .keybindings
                .insert("View: Zoom In".into(), Some("alt-z".into()));
            crate::settings::apply_keybindings(&prefs, cx);
        });
        visual.simulate_keystrokes(&format!("{modifier}-="));
        window
            .update(visual, |view, _, _| assert_eq!(view.view, View::IDENTITY))
            .unwrap();
        visual.simulate_keystrokes("alt-z");
        window
            .update(visual, |view, _, _| {
                assert_eq!(view.view.scale, KEY_ZOOM_STEP)
            })
            .unwrap();
    }
}
