//! Prepare the React worker's texture once. Cover cropping, corner clipping and
//! view-relative masks happen while painting, including during live resize.
use crate::popover::AnchorSlot;
use gpui::{
    App, Asset, Bounds, Context, ObjectFit, RenderImage, Role, Window, canvas, div, img,
    prelude::*, px,
};
#[cfg(any(target_os = "macos", test))]
use gpui::{Corners, Pixels};
use image::Frame;
use std::{
    hash::{Hash, Hasher},
    path::Path,
    sync::Arc,
};
#[cfg(target_os = "macos")]
use vorkspace_core::project_appearance::background_texture;
use vorkspace_core::project_appearance::{Background, Effect, read_image};

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    path: String,
    effect: Effect,
    #[cfg(not(target_os = "macos"))]
    empty: bool,
    #[cfg(not(target_os = "macos"))]
    preview: bool,
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
    #[cfg(target_os = "macos")]
    let pixels = background_texture(&source, request.key.effect);
    #[cfg(not(target_os = "macos"))]
    let pixels = vorkspace_core::project_appearance::software_background_texture(
        &source,
        request.key.effect,
        request.key.empty,
        request.key.preview,
    );
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
                #[cfg(target_os = "macos")]
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
                #[cfg(not(target_os = "macos"))]
                if !begin {
                    // Other backends have an opaque chat surface. Reveal that
                    // surface with the same view-relative stops while it resizes.
                    use crate::theme::CHAT;
                    use vorkspace_core::project_appearance::background_fade;
                    let _ = (preview, &clip, opacity);
                    let height = bounds.size.height.as_f32().ceil() as usize;
                    for y in 0..height {
                        let mut color: gpui::Hsla = gpui::rgb(CHAT).into();
                        color.a =
                            1. - background_fade((y as f32 + 0.5) / height as f32, empty, haze);
                        window.paint_quad(gpui::fill(
                            Bounds::new(
                                gpui::point(bounds.left(), bounds.top() + px(y as f32)),
                                gpui::size(bounds.size.width, px(1.)),
                            ),
                            color,
                        ));
                    }
                }
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
#[cfg(any(target_os = "macos", test))]
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
            #[cfg(not(target_os = "macos"))]
            empty: settings.effect != Effect::GradientBlur || self.empty,
            #[cfg(not(target_os = "macos"))]
            preview: settings.effect == Effect::GradientBlur && self.preview,
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
                    .child(img(image).size_full().object_fit(ObjectFit::Cover).when(
                        cfg!(not(target_os = "macos")),
                        |image| {
                            image.opacity(settings.opacity(self.empty)).rounded(px(
                                if self.preview {
                                    11.
                                } else if self.clip.is_some() {
                                    9.
                                } else {
                                    0.
                                },
                            ))
                        },
                    ))
                    .child(self.mask(false, &settings))
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, point, size};

    #[test]
    fn artwork_rounds_only_the_corners_on_the_card_perimeter() {
        let card = Bounds::new(point(px(20.), px(40.)), size(px(800.), px(600.)));
        let inset = Bounds::new(point(px(21.), px(41.)), size(px(798.), px(598.)));
        assert_eq!(corners(inset, false, Some(card)), Corners::all(px(9.)));
        let beside_review = Bounds::new(inset.origin, size(px(400.), inset.size.height));
        let rounded = corners(beside_review, false, Some(card));
        assert_eq!(rounded.top_left, px(9.));
        assert_eq!(rounded.bottom_left, px(9.));
        assert_eq!(rounded.top_right, px(0.));
        let below_header = Bounds::new(point(px(21.), px(70.)), size(px(798.), px(569.)));
        assert_eq!(corners(below_header, false, Some(card)).top_left, px(0.));
        assert_eq!(corners(inset, false, None), Corners::default());
        assert_eq!(corners(inset, true, None), Corners::all(px(11.)));
    }

    #[gpui::test]
    fn new_views_share_pending_artwork_and_draw_warm_images_on_the_first_frame(
        cx: &mut TestAppContext,
    ) {
        struct Surface(Vec<gpui::Entity<ProjectBackground>>);
        impl Render for Surface {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .relative()
                    .size_full()
                    .children(self.0.iter().cloned())
            }
        }
        let settings = Background {
            path: concat!(env!("CARGO_MANIFEST_DIR"), "/assets/vorkspace.png").into(),
            effect: Effect::Dither,
            ..Default::default()
        };
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| {
                    Surface(
                        (0..2)
                            .map(|_| {
                                cx.new(|_| ProjectBackground {
                                    settings: Some(settings.clone()),
                                    empty: true,
                                    ..Default::default()
                                })
                            })
                            .collect(),
                    )
                })
            })
            .unwrap()
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.refresh().unwrap();
        cx.run_until_parked();
        let image = window
            .update(cx, |surface, _, cx| {
                let first = surface.0[0].read(cx).image.as_ref().unwrap();
                let second = surface.0[1].read(cx).image.as_ref().unwrap();
                assert!(
                    Arc::ptr_eq(first, second),
                    "concurrent views must share a single preparation job"
                );
                first.clone()
            })
            .unwrap();
        let warm = window
            .update(cx, |surface, _, cx| {
                // Drop the original views, as Close All does before its new tab
                // appears. The application's prepared artwork remains available.
                surface.0.clear();
                let warm = cx.new(|_| ProjectBackground {
                    preview: true,
                    settings: Some(Background {
                        empty_opacity: 0.24,
                        ..settings.clone()
                    }),
                    empty: true,
                    ..Default::default()
                });
                surface.0.push(warm.clone());
                cx.notify();
                warm
            })
            .unwrap();
        // Draw exactly once without giving a new worker a chance to finish.
        cx.update_window(window.into(), |_, window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        })
        .unwrap();
        warm.read_with(cx, |background, _cx| {
            assert!(!background.loading);
            assert!(Arc::ptr_eq(background.image.as_ref().unwrap(), &image));
            assert!(image.size(0).width.0 > 0);
        });
        warm.update(cx, |background, cx| {
            background.configure(
                Some(Background {
                    effect: Effect::None,
                    ..settings.clone()
                }),
                true,
                None,
                cx,
            );
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.refresh().unwrap();
        cx.run_until_parked();
        warm.read_with(cx, |background, _| {
            assert!(
                !Arc::ptr_eq(background.image.as_ref().unwrap(), &image),
                "different effects need different textures"
            );
        });
        warm.update(cx, |background, cx| {
            background.configure(Some(settings), true, None, cx)
        });
        cx.update_window(window.into(), |_, window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        })
        .unwrap();
        warm.read_with(cx, |background, _| {
            assert!(!background.loading);
            assert!(
                Arc::ptr_eq(background.image.as_ref().unwrap(), &image),
                "returning to an effect reuses its prepared texture immediately"
            );
        });
    }

    #[gpui::test]
    fn live_resize_and_opacity_reuse_the_texture_without_a_background_job(cx: &mut TestAppContext) {
        struct Surface(gpui::Entity<ProjectBackground>);
        impl Render for Surface {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().relative().size_full().child(self.0.clone())
            }
        }
        let image = render_image(background_texture(
            &read_image(Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/vorkspace.png"
            )))
            .unwrap(),
            Effect::Dither,
        ));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| {
                    Surface(cx.new(|_| ProjectBackground {
                        preview: true,
                        settings: Some(Background {
                            path: "prepared-artwork".into(),
                            effect: Effect::Dither,
                            ..Default::default()
                        }),
                        empty: true,
                        key: Some(Key {
                            path: "prepared-artwork".into(),
                            effect: Effect::Dither,
                            #[cfg(not(target_os = "macos"))]
                            empty: true,
                            #[cfg(not(target_os = "macos"))]
                            preview: false,
                        }),
                        image: Some(image.clone()),
                        ..Default::default()
                    }))
                })
            })
            .unwrap()
        });
        for dimensions in [size(px(640.), px(900.)), size(px(1400.), px(650.))] {
            cx.simulate_window_resize(window.into(), dimensions);
            cx.refresh().unwrap();
            window
                .update(cx, |surface, _window, cx| {
                    surface.0.update(cx, |background, cx| {
                        assert!(Arc::ptr_eq(background.image.as_ref().unwrap(), &image));
                        assert!(
                            !background.loading,
                            "resize must not schedule image processing"
                        );
                        #[cfg(target_os = "macos")]
                        {
                            let marker = gpui_apple::metal_renderer::background_mask_marker(
                                false, true, false, true, 0.24,
                            );
                            let quads = _window.painted_quads();
                            let mask = quads
                                .iter()
                                .find(|q| q.border_color == marker)
                                .unwrap_or_else(|| {
                                    panic!(
                                        "missing background mask in {} painted quads: {:?}",
                                        quads.len(),
                                        quads
                                            .iter()
                                            .map(|q| (q.bounds, q.border_color))
                                            .collect::<Vec<_>>()
                                    )
                                });
                            assert_eq!(
                                mask.bounds.origin,
                                gpui::point(gpui::ScaledPixels(0.), gpui::ScaledPixels(0.))
                            );
                            assert_eq!(
                                mask.bounds.size.width.0,
                                dimensions.width.as_f32() * _window.scale_factor()
                            );
                            assert_eq!(
                                mask.bounds.size.height.0,
                                dimensions.height.as_f32() * _window.scale_factor()
                            );
                            assert_eq!(mask.corner_radii.top_left.0, 11. * _window.scale_factor());
                        }
                        let mut settings = background.settings.clone().unwrap();
                        settings.empty_opacity = 0.24;
                        settings.session_opacity = 0.33;
                        background.configure(Some(settings), true, None, cx);
                    })
                })
                .unwrap();
        }
    }
}
