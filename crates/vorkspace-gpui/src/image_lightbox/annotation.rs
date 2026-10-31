//! Image-relative strokes and the flattened, private attachment representation.
use image::{AnimationDecoder, ImageDecoder, ImageReader, RgbaImage};
use std::{
    io::{Cursor, Read},
    path::Path,
};

#[derive(Clone, Debug)]
pub(super) struct Stroke {
    pub color: u32,
    /// Width and positions are fractions of the image width/height respectively.
    pub width: f32,
    pub points: Vec<(f32, f32)>,
}

#[derive(Clone, Debug)]
pub(super) struct TextBox {
    pub position: (f32, f32),
    /// Font size is relative to image width, just like brush width.
    pub size: f32,
    pub color: u32,
    pub text: String,
}

pub(super) fn read_image(path: &Path) -> Result<RgbaImage, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(vorkspace_core::composer::MAX_EMBED_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > vorkspace_core::composer::MAX_EMBED_BYTES {
        return Err("Image exceeds 20 MiB".into());
    }
    let format = image::guess_format(&bytes).map_err(|e| e.to_string())?;
    // Keep animated previews animated; a flattened first frame would misrepresent
    // the attachment. Animated images remain viewable but cannot be annotated.
    if format == image::ImageFormat::Gif {
        let mut decoder =
            image::codecs::gif::GifDecoder::new(Cursor::new(&bytes)).map_err(|e| e.to_string())?;
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(256 * 1024 * 1024);
        limits.max_image_width = Some(16384);
        limits.max_image_height = Some(16384);
        decoder.set_limits(limits).map_err(|e| e.to_string())?;
        let (width, height) = decoder.dimensions();
        if u64::from(width) * u64::from(height) > 40_000_000 {
            return Err("Image is too large to annotate".into());
        }
        if decoder.into_frames().take(2).count() > 1 {
            return Err("Animated images can be previewed but not annotated".into());
        }
    } else if format == image::ImageFormat::WebP {
        let decoder = image::codecs::webp::WebPDecoder::new(Cursor::new(&bytes))
            .map_err(|e| e.to_string())?;
        if decoder.has_animation() {
            return Err("Animated images can be previewed but not annotated".into());
        }
    }
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(256 * 1024 * 1024);
    decoder.set_limits(limits).map_err(|e| e.to_string())?;
    let (w, h) = decoder.dimensions();
    if u64::from(w) * u64::from(h) > 40_000_000 {
        return Err("Image is too large to annotate".into());
    }
    let orientation = decoder.orientation().map_err(|e| e.to_string())?;
    let mut image = image::DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    image.apply_orientation(orientation);
    Ok(image.into_rgba8())
}

pub(super) fn png(
    image: &RgbaImage,
    strokes: &[Stroke],
    texts: &[TextBox],
) -> Result<Vec<u8>, String> {
    let mut image = image.clone();
    let (w, h) = image.dimensions();
    for stroke in strokes {
        let radius = (stroke.width * w as f32 / 2.).max(0.5);
        let color = image::Rgba([
            (stroke.color >> 16) as u8,
            (stroke.color >> 8) as u8,
            stroke.color as u8,
            255,
        ]);
        let draw_dot = |image: &mut RgbaImage, x: f32, y: f32| {
            let left = (x - radius - 1.).floor().max(0.) as u32;
            let right = (x + radius + 1.).ceil().min(w as f32) as u32;
            let top = (y - radius - 1.).floor().max(0.) as u32;
            let bottom = (y + radius + 1.).ceil().min(h as f32) as u32;
            for py in top..bottom {
                for px in left..right {
                    let distance =
                        ((px as f32 + 0.5 - x).powi(2) + (py as f32 + 0.5 - y).powi(2)).sqrt();
                    let coverage = (radius + 0.5 - distance).clamp(0., 1.);
                    if coverage > 0. {
                        let dest = image.get_pixel_mut(px, py);
                        // Source-over compositing, including transparent screenshots.
                        let a = coverage;
                        let old_a = dest[3] as f32 / 255.;
                        let out_a = a + old_a * (1. - a);
                        for c in 0..3 {
                            dest[c] = ((color[c] as f32 * a + dest[c] as f32 * old_a * (1. - a))
                                / out_a)
                                .round() as u8;
                        }
                        dest[3] = (out_a * 255.).round() as u8;
                    }
                }
            }
        };
        let scaled = |p: (f32, f32)| (p.0 * w as f32, p.1 * h as f32);
        if let Some(&p) = stroke.points.first() {
            let (x, y) = scaled(p);
            draw_dot(&mut image, x, y);
        }
        for segment in stroke.points.windows(2) {
            let (x0, y0) = scaled(segment[0]);
            let (x1, y1) = scaled(segment[1]);
            let steps = ((x1 - x0).hypot(y1 - y0) / (radius * 0.5).max(0.5)).ceil() as usize;
            for i in 1..=steps {
                let t = i as f32 / steps as f32;
                draw_dot(&mut image, x0 + (x1 - x0) * t, y0 + (y1 - y0) * t);
            }
        }
    }
    if !texts.is_empty() {
        // GPUI's macOS editor uses this same CoreText shaper and glyph rasterizer.
        // Keep each instance's font IDs local: fallback runs may introduce new IDs.
        let fonts = gpui_apple::AppleTextSystem::new();
        for text in texts.iter().filter(|text| !text.text.is_empty()) {
            draw_text(&mut image, text, &fonts)?;
        }
    }
    let mut output = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut output, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(output.into_inner())
}

// The editor and export deliberately use a single unwrapped system-font line.
// Its origin is the top-left of the line box, in image-relative coordinates.
pub(super) const FONT_FAMILY: &str = crate::fonts::UI;
const CLIPPED: &str = "Text extends beyond the image; shorten it or place it farther from the edge";

fn draw_text(
    image: &mut RgbaImage,
    text: &TextBox,
    fonts: &impl gpui::PlatformTextSystem,
) -> Result<(), String> {
    use gpui::{FontRun, RenderGlyphParams, point, px};

    if text.text.contains(['\r', '\n']) {
        return Err("Annotation text must be a single line".into());
    }
    let (width, height) = image.dimensions();
    let font_size = px((text.size * width as f32).max(1.));
    let line_height = font_size * crate::input::ANNOTATION_LINE_HEIGHT;
    let font_id = fonts
        .font_id(&gpui::font(FONT_FAMILY))
        .map_err(|e| e.to_string())?;
    let layout = fonts.layout_line(
        &text.text,
        font_size,
        &[FontRun {
            len: text.text.len(),
            font_id,
        }],
    );
    let origin_x = text.position.0 * width as f32;
    let origin_y = text.position.1 * height as f32;
    if !origin_x.is_finite()
        || !origin_y.is_finite()
        || origin_x < 0.
        || origin_y < 0.
        || origin_x + layout.width.as_f32() > width as f32
        || origin_y + line_height.as_f32() > height as f32
    {
        return Err(CLIPPED.into());
    }
    let baseline =
        origin_y + ((line_height - layout.ascent - layout.descent) / 2. + layout.ascent).as_f32();
    let color = gpui::rgb(text.color);
    let dilation = fonts.glyph_dilation_for_color(color.into());
    for run in &layout.runs {
        for glyph in &run.glyphs {
            let x = origin_x + glyph.position.x.as_f32();
            let y = baseline + glyph.position.y.as_f32();
            // Match Window::paint_glyph's device-pixel quantization at scale 1;
            // PNG pixels are the target device pixels. Emoji use integer origins.
            let quantize = |value: f32, variants: u8| {
                let scaled = value * variants as f32;
                (scaled.abs() - 0.5).ceil().copysign(scaled) / variants as f32
            };
            let (x, y) = if glyph.is_emoji {
                (quantize(x, 1), quantize(y, 1))
            } else {
                (
                    quantize(x, gpui::SUBPIXEL_VARIANTS_X),
                    quantize(y, gpui::SUBPIXEL_VARIANTS_Y),
                )
            };
            let params = RenderGlyphParams {
                font_id: run.font_id,
                glyph_id: glyph.id,
                font_size,
                subpixel_variant: if glyph.is_emoji {
                    point(0, 0)
                } else {
                    point(
                        (x.fract() * gpui::SUBPIXEL_VARIANTS_X as f32) as u8,
                        (y.fract() * gpui::SUBPIXEL_VARIANTS_Y as f32) as u8,
                    )
                },
                scale_factor: 1.,
                is_emoji: glyph.is_emoji,
                subpixel_rendering: false,
                dilation: if glyph.is_emoji { 0 } else { dilation },
            };
            let bounds = fonts
                .glyph_raster_bounds(&params)
                .map_err(|e| e.to_string())?;
            if bounds.size.width.0 == 0 || bounds.size.height.0 == 0 {
                continue;
            }
            let (size, pixels) = fonts
                .rasterize_glyph(&params, bounds)
                .map_err(|e| e.to_string())?;
            let left = x.trunc() as i32 + bounds.origin.x.0;
            let top = y.trunc() as i32 + bounds.origin.y.0;
            let channels = if glyph.is_emoji { 4 } else { 1 };
            for row in 0..size.height.0 as usize {
                for column in 0..size.width.0 as usize {
                    let offset = (row * size.width.0 as usize + column) * channels;
                    // AppleTextSystem returns straight-alpha BGRA for emoji and
                    // alpha coverage for ordinary glyphs, just like GPUI's atlas.
                    let (source, alpha) = if glyph.is_emoji {
                        (
                            [pixels[offset + 2], pixels[offset + 1], pixels[offset]],
                            pixels[offset + 3],
                        )
                    } else {
                        (
                            [
                                (text.color >> 16) as u8,
                                (text.color >> 8) as u8,
                                text.color as u8,
                            ],
                            pixels[offset],
                        )
                    };
                    if alpha == 0 {
                        continue;
                    }
                    let x = left + column as i32;
                    let y = top + row as i32;
                    if (x as f32) < origin_x.floor()
                        || (y as f32) < origin_y.floor()
                        || x >= width as i32
                        || y >= height as i32
                        || (y as f32) >= (origin_y + line_height.as_f32()).ceil()
                    {
                        return Err(CLIPPED.into());
                    }
                    let dest = image.get_pixel_mut(x as u32, y as u32);
                    let a = alpha as f32 / 255.;
                    let old_a = dest[3] as f32 / 255.;
                    let out_a = a + old_a * (1. - a);
                    for channel in 0..3 {
                        dest[channel] = ((source[channel] as f32 * a
                            + dest[channel] as f32 * old_a * (1. - a))
                            / out_a)
                            .round() as u8;
                    }
                    dest[3] = (out_a * 255.).round() as u8;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_still_formats_decode_and_animated_gif_stays_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            12,
            8,
            image::Rgb([20, 30, 40]),
        ));
        for (format, extension) in [
            (image::ImageFormat::Png, "png"),
            (image::ImageFormat::Jpeg, "jpg"),
            (image::ImageFormat::Gif, "gif"),
            (image::ImageFormat::WebP, "webp"),
        ] {
            let path = dir.path().join(format!("image.{extension}"));
            image.save_with_format(&path, format).unwrap();
            let decoded = read_image(&path).unwrap();
            assert_eq!(decoded.dimensions(), (12, 8));
        }
        let path = dir.path().join("animated.gif");
        let file = std::fs::File::create(&path).unwrap();
        let mut encoder = image::codecs::gif::GifEncoder::new(file);
        for _ in 0..2 {
            encoder
                .encode_frame(image::Frame::new(image.to_rgba8()))
                .unwrap();
        }
        drop(encoder);
        assert!(read_image(&path).unwrap_err().contains("Animated"));
        assert!(read_image(&dir.path().join("missing.png")).is_err());
    }

    #[test]
    fn flattened_strokes_preserve_source_and_apply_selected_color_and_alpha() {
        let original = RgbaImage::from_pixel(100, 50, image::Rgba([255, 255, 255, 255]));
        let strokes = vec![
            Stroke {
                color: 0xff3b30,
                width: 0.06,
                points: vec![(0.2, 0.5), (0.8, 0.5)],
            },
            Stroke {
                color: 0x0055ff,
                width: 0.06,
                points: vec![(0.5, 0.2)],
            },
        ];
        let bytes = png(&original, &strokes, &[]).unwrap();
        let flattened = image::load_from_memory(&bytes).unwrap().into_rgba8();
        assert_eq!(original.get_pixel(50, 25).0, [255, 255, 255, 255]);
        assert_eq!(flattened.get_pixel(50, 25).0, [255, 59, 48, 255]);
        assert_eq!(flattened.get_pixel(50, 10).0, [0, 85, 255, 255]);
        assert_eq!(flattened.get_pixel(5, 5).0, [255, 255, 255, 255]);
        assert_eq!(
            image::load_from_memory(&png(&original, &[], &[]).unwrap())
                .unwrap()
                .into_rgba8(),
            original
        );
    }
    #[test]
    fn system_layout_uses_proportional_and_fallback_runs_and_checks_edges() {
        use gpui::{FontRun, PlatformTextSystem, px};
        let fonts = gpui_apple::AppleTextSystem::new();
        let id = fonts.font_id(&gpui::font(FONT_FAMILY)).unwrap();
        let layout = |text: &str| {
            fonts.layout_line(
                text,
                px(24.),
                &[FontRun {
                    len: text.len(),
                    font_id: id,
                }],
            )
        };
        assert!(layout("WWWW").width > layout("iiii").width);
        let fallback = layout("Wi 漢 🐙");
        assert!(fallback.runs.iter().any(|run| run.font_id != id));
        assert!(
            fallback
                .runs
                .iter()
                .flat_map(|run| &run.glyphs)
                .any(|glyph| glyph.is_emoji)
        );
        let original = RgbaImage::from_pixel(320, 120, image::Rgba([255, 255, 255, 255]));
        let text = TextBox {
            position: (0.8, 0.7),
            size: 0.06,
            color: 0x000000,
            text: "Wi".into(),
        };
        let saved =
            image::load_from_memory(&png(&original, &[], std::slice::from_ref(&text)).unwrap())
                .unwrap()
                .into_rgba8();
        assert!(saved.pixels().any(|pixel| pixel[0] < 200));
        let colored = TextBox {
            position: (0.1, 0.2),
            size: 0.08,
            color: 0x000000,
            text: "漢 🐙".into(),
        };
        let saved = image::load_from_memory(&png(&original, &[], &[colored]).unwrap())
            .unwrap()
            .into_rgba8();
        assert!(
            saved
                .pixels()
                .any(|pixel| pixel[0] != pixel[1] || pixel[1] != pixel[2])
        );
        assert!(
            png(
                &original,
                &[],
                &[TextBox {
                    position: (0.99, 0.7),
                    ..text.clone()
                }]
            )
            .is_err()
        );
        assert!(
            png(
                &original,
                &[],
                &[TextBox {
                    position: (0.8, 0.99),
                    ..text
                }]
            )
            .is_err()
        );
        assert!(
            original
                .pixels()
                .all(|pixel| pixel.0 == [255, 255, 255, 255])
        );
    }

    #[test]
    fn text_flattens_with_brush_and_rejects_clipped_content() {
        let original = RgbaImage::from_pixel(400, 160, image::Rgba([255, 255, 255, 255]));
        let text = TextBox {
            position: (0.1, 0.25),
            size: 0.08,
            color: 0x0055ff,
            text: "Hello".into(),
        };
        let stroke = Stroke {
            color: 0xff3b30,
            width: 0.02,
            points: vec![(0.7, 0.7)],
        };
        let flattened = image::load_from_memory(
            &png(&original, &[stroke], std::slice::from_ref(&text)).unwrap(),
        )
        .unwrap()
        .into_rgba8();
        assert_eq!(flattened.get_pixel(280, 112).0, [255, 59, 48, 255]);
        assert!(flattened.pixels().any(|p| p[2] > p[0] && p[2] > p[1]));
        assert_eq!(original.get_pixel(280, 112).0, [255, 255, 255, 255]);
        let clipped = TextBox {
            position: (0.99, 0.25),
            ..text
        };
        assert!(
            png(&original, &[], &[clipped])
                .unwrap_err()
                .contains("beyond the image")
        );
    }
}
