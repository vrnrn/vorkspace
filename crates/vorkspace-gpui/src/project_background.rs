//! Prepare the React worker's texture once. Cover cropping, corner clipping and
//! view-relative masks happen while painting, including during live resize.
use crate::popover::AnchorSlot;
use gpui::{
    App, Asset, Bounds, Context, ObjectFit, RenderImage, Role, Window, canvas, div, img,
    prelude::*, px,
};
use gpui::{Corners, Pixels};
use image::Frame;
use std::{
    hash::{Hash, Hasher},
    path::Path,
    sync::Arc,
};
use vorkspace_core::project_appearance::background_texture;
use vorkspace_core::project_appearance::{Background, Effect, read_image};

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    path: String,
    effect: Effect,
}

#[derive(Clone)]
struct TextureRequest {
    key: Key,
}
impl Hash for TextureRequest {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Imported images have content-addressed paths. Pixels, effects and
        // decoding are shared regardless of which tab already read the source.
        self.key.hash(state);
    }
}
#[derive(Clone)]
struct PreparedTexture {
    image: Arc<RenderImage>,
}
struct BackgroundTexture;
impl Asset for BackgroundTexture {
    type Source = TextureRequest;
    type Output = Result<PreparedTexture, String>;

    fn load(
        request: TextureRequest,
        _: &mut App,
    ) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        prepare_texture(request)
    }
}
// Keep the UI-thread App out of the Send future run by the asset cache.
async fn prepare_texture(request: TextureRequest) -> Result<PreparedTexture, String> {
    let source = read_image(Path::new(&request.key.path))?;
    let pixels = background_texture(&source, request.key.effect);
    Ok(PreparedTexture {
        image: render_image(pixels),
    })
}

fn render_image(mut pixels: image::RgbaImage) -> Arc<RenderImage> {
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(RenderImage::new(vec![Frame::new(pixels)]))
}
#[derive(Default)]
pub(crate) struct ProjectBackground {
    preview: bool,
    settings: Option<Background>,
    empty: bool,
    clip: Option<AnchorSlot>,
    key: Option<Key>,
    image: Option<Arc<RenderImage>>,
    pub(crate) error: Option<String>,
    loading: bool,
}
impl ProjectBackground {
    #[cfg(test)]
    pub(crate) fn prepared_image(&self) -> Option<Arc<RenderImage>> {
        self.image.clone()
    }
    pub(crate) fn preview() -> Self {
        Self {
            preview: true,
            ..Self::default()
        }
    }
    pub(crate) fn configure(
        &mut self,
        settings: Option<Background>,
        empty: bool,
        clip: Option<AnchorSlot>,
        cx: &mut Context<Self>,
    ) {
        let clip_changed = self.clip.is_some() != clip.is_some();
        self.clip = clip;
        if self.settings == settings && self.empty == empty && !clip_changed {
            return;
        }
        if self.settings.as_ref().map(|s| &s.path) != settings.as_ref().map(|s| &s.path) {
            self.loading = false;
            self.image = None;
            self.key = None;
            self.error = None;
        }
        self.settings = settings;
        self.empty = empty;
        cx.notify();
    }
    fn prepare(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) {
        self.key = Some(key.clone());
        self.error = None;
        // GPUI shares pending jobs and completed textures across views. A warm
        // hit returns in this render, with the same prepared render sprite.
        // Closing the first tab cannot cancel a load another tab is waiting on.
        match window.use_asset::<BackgroundTexture>(&TextureRequest { key }, cx) {
            Some(Ok(prepared)) => {
                self.image = Some(prepared.image);
                self.loading = false;
            }
            Some(Err(error)) => {
                self.image = None;
                self.error = Some(error);
                self.loading = false;
            }
            None => self.loading = true,
        }
    }
    fn mask(&self, begin: bool, settings: &Background) -> impl IntoElement {
        let clip = self.clip.clone();
        let preview = self.preview;
        let empty = self.empty;
        let opacity = settings.opacity(empty);
        let haze = settings.effect == Effect::GradientBlur;
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                window.paint_quad(
                    gpui::fill(bounds, gpui::black())
                        .corner_radii(corners(
                            bounds,
                            preview,
                            clip.as_ref().and_then(AnchorSlot::get),
                        ))
                        .border_widths(gpui::Edges {
                            left: px(1.),
                            ..Default::default()
                        })
                        .border_color(gpui_apple::metal_renderer::background_mask_marker(
                            begin, empty, haze, preview, opacity,
                        )),
                );
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }
}

// Round only corners that meet the inset card's perimeter. Split headers,
// review panes and terminal docks retain their straight interior boundaries.
fn corners(bounds: Bounds<Pixels>, preview: bool, card: Option<Bounds<Pixels>>) -> Corners<Pixels> {
    if preview {
        return Corners::all(px(11.));
    }
    let Some(card) = card else {
        return Corners::default();
    };
    let near = |a: Pixels, b: Pixels| (a - b).abs() <= px(2.);
    let radius = px(9.)
        .min(bounds.size.width / 2.)
        .min(bounds.size.height / 2.);
    let left = near(bounds.left(), card.left());
    let right = near(bounds.right(), card.right());
    let top = near(bounds.top(), card.top());
    let bottom = near(bounds.bottom(), card.bottom());
    Corners {
        top_left: if top && left { radius } else { px(0.) },
        top_right: if top && right { radius } else { px(0.) },
        bottom_right: if bottom && right { radius } else { px(0.) },
        bottom_left: if bottom && left { radius } else { px(0.) },
    }
}
impl Render for ProjectBackground {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(settings) = self.settings.clone().filter(|s| s.opacity(self.empty) > 0.) else {
            return div().into_any_element();
        };
        let key = Key {
            path: settings.path.clone(),
            effect: settings.effect,
        };
        if self.key.as_ref() != Some(&key) || self.loading {
            self.prepare(key, window, cx);
        }
        div()
            .id("project-chat-background")
            .debug_selector(|| "project-chat-background".into())
            .role(Role::Image)
            .aria_label("Project background")
            .absolute()
            .size_full()
            .overflow_hidden()
            .when_some(self.image.clone(), |d, image| {
                d.child(self.mask(true, &settings))
                    .child(img(image).size_full().object_fit(ObjectFit::Cover))
                    .child(self.mask(false, &settings))
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests;
