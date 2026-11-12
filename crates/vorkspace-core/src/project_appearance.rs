//! Project appearance and the image treatments from React's background worker.
use image::{imageops, ImageReader, RgbaImage};
use serde::{Deserialize, Serialize};
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    io::Cursor,
    path::{Path, PathBuf},
};

mod bot_body;

pub use bot_body::render_bot_body;

pub const COLORS: [u32; 9] = [
    0x8b949c, 0x459cf7, 0xea603e, 0xf4bf25, 0x39c363, 0xe45a9f, 0xb168d4, 0x37beb4, 0xef8c39,
];
pub const MASCOTS: [&str; 10] = [
    "invader", "ghost", "robot", "cat", "skull", "crab", "mushroom", "rocket", "dino", "frog",
];
pub const BADGE_SHAPES: [&str; 8] = [
    "squircle", "circle", "rounded", "pill", "hex", "octagon", "ghost", "tv",
];
/// Eye styles. Expressions (a wink, happy carets, crossed eyes) are moods the
/// face can wear for a moment, not styles to choose.
pub const BOT_FACES: [&str; 6] = ["pills", "dots", "visor", "square", "slits", "rings"];
/// A Bot's body pattern. "rings" is the legacy concentric texture projects and
/// channels keep; Bots choose among the printed patterns, and an unknown or
/// legacy value reads as "dither".
pub const BOT_PATTERNS: [&str; 5] = ["dither", "halftone", "grain", "stripes", "solid"];

pub(crate) fn normalize_bot_print(print: &str) -> &'static str {
    match print {
        "cap" => "cap",
        "full" => "full",
        _ => "ground",
    }
}

pub(crate) fn normalize_bot_pattern(pattern: &str) -> &'static str {
    match pattern {
        "halftone" => "halftone",
        "grain" => "grain",
        "stripes" => "stripes",
        "solid" => "solid",
        "rings" => "rings",
        _ => "dither",
    }
}

/// Curated background + foreground palette pairs for concentric texture rendering.
pub const TEXTURE_PALETTES: [(&str, u32, u32); 8] = [
    ("Cobalt", 0x3b82f6, 0x0b1528),
    ("Emerald", 0x22c55e, 0x052e16),
    ("Plum", 0xa855f7, 0x230738),
    ("Rose", 0xec4899, 0x38061e),
    ("Amber", 0xf59e0b, 0x271703),
    ("Teal", 0x14b8a6, 0x042f2e),
    ("Obsidian", 0x1c1c21, 0xededf2),
    ("Charcoal", 0x2c2c34, 0xa1a1aa),
];

/// Derive a dark companion foreground color from a background hex color.
pub fn default_foreground_color(bg: u32) -> u32 {
    for &(_, p_bg, p_fg) in &TEXTURE_PALETTES {
        if p_bg == bg {
            return p_fg;
        }
    }
    let r = ((bg >> 16) & 0xff) as f32;
    let g = ((bg >> 8) & 0xff) as f32;
    let b = (bg & 0xff) as f32;
    let lum = (0.299 * r + 0.587 * g + 0.114 * b) / 255.0;
    if lum < 0.18 {
        let nr = (r + (255.0 - r) * 0.72).round().clamp(0.0, 255.0) as u32;
        let ng = (g + (255.0 - g) * 0.72).round().clamp(0.0, 255.0) as u32;
        let nb = (b + (255.0 - b) * 0.72).round().clamp(0.0, 255.0) as u32;
        (nr << 16) | (ng << 8) | nb
    } else {
        let nr = (r * 0.18).round().clamp(0.0, 255.0) as u32;
        let ng = (g * 0.18).round().clamp(0.0, 255.0) as u32;
        let nb = (b * 0.22).round().clamp(0.0, 255.0) as u32;
        (nr << 16) | (ng << 8) | nb
    }
}

/// Derive a contrasting face color when texture is disabled (single-color mode).
pub fn auto_contrast_color(bg: u32) -> u32 {
    let r = ((bg >> 16) & 0xff) as f32;
    let g = ((bg >> 8) & 0xff) as f32;
    let b = (bg & 0xff) as f32;
    let lum = (0.299 * r + 0.587 * g + 0.114 * b) / 255.0;
    if lum > 0.36 {
        0x0d1117
    } else {
        0xf4f4f7
    }
}

/// Deterministic 5-digit seed derived from a stable string key (10000..=99999).
pub fn default_seed_for(key: &str) -> u32 {
    let hash = key.encode_utf16().fold(2166136261u32, |h, c| {
        (h ^ u32::from(c)).wrapping_mul(16777619)
    });
    10000 + (hash % 90000)
}

/// Resolve current face styles and saved mascot names.
pub fn normalize_bot_face(face_or_mascot: &str) -> &'static str {
    match face_or_mascot {
        "pills" | "invader" | "skull" => "pills",
        "dots" | "ghost" | "mushroom" => "dots",
        "visor" | "robot" | "rocket" => "visor",
        "square" => "square",
        "slits" | "carets" | "cat" | "crab" => "slits",
        "rings" | "wink" | "dino" | "frog" => "rings",
        _ => "pills",
    }
}

/// Resolve the saved body shape.
pub fn normalize_badge_shape(shape: &str) -> &'static str {
    match shape {
        "squircle" => "squircle",
        "circle" => "circle",
        "rounded" => "rounded",
        "pill" => "pill",
        "hex" => "hex",
        "octagon" => "octagon",
        "ghost" => "ghost",
        "tv" => "tv",
        _ => "squircle",
    }
}

/// Stable folder color keyed by the project path.
pub fn project_color(project: &str) -> u32 {
    let hash = project.encode_utf16().fold(0u32, |hash, unit| {
        hash.wrapping_mul(31).wrapping_add(unit.into())
    });
    COLORS[(hash as usize % (COLORS.len() - 1)) + 1]
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Effect {
    #[default]
    None,
    Dither,
    Ascii,
    Halftone,
    Scanlines,
    GradientBlur,
}
impl Effect {
    pub const ALL: [Self; 6] = [
        Self::None,
        Self::Dither,
        Self::Ascii,
        Self::Halftone,
        Self::Scanlines,
        Self::GradientBlur,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Dither => "Dither",
            Self::Ascii => "ASCII",
            Self::Halftone => "Halftone",
            Self::Scanlines => "Scanlines",
            Self::GradientBlur => "Haze",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::None => "Shows the original artwork.",
            Self::Dither => "Rebuilds the artwork with a dithered color palette.",
            Self::Ascii => "Recreates the artwork with colored characters on black.",
            Self::Halftone => "Recreates the artwork with colored print dots on black.",
            Self::Scanlines => "Adds a pronounced horizontal display-line texture.",
            Self::GradientBlur => "Blurs and fades the artwork into the background below.",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Empty,
    #[default]
    All,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Background {
    pub path: String,
    pub effect: Effect,
    pub scope: Scope,
    pub empty_opacity: f32,
    pub session_opacity: f32,
}
impl Default for Background {
    fn default() -> Self {
        Self {
            path: String::new(),
            effect: Effect::None,
            scope: Scope::All,
            empty_opacity: 0.24,
            session_opacity: 0.24,
        }
    }
}
impl Background {
    pub fn normalize(&mut self) {
        for value in [&mut self.empty_opacity, &mut self.session_opacity] {
            *value = if value.is_finite() {
                value.clamp(0.05, 0.65)
            } else {
                0.24
            };
        }
    }
    pub fn opacity(&self, empty: bool) -> f32 {
        if !empty && self.scope == Scope::Empty {
            0.
        } else if empty {
            self.empty_opacity
        } else {
            self.session_opacity
        }
    }
}

/// Decode with bounded allocations before resizing; all callers run off-thread.
pub fn read_image(path: &Path) -> Result<RgbaImage, String> {
    let metadata = std::fs::metadata(path).map_err(|e| format!("Cannot read image: {e}"))?;
    if metadata.len() > 30 * 1024 * 1024 {
        return Err("Choose an image smaller than 30 MB.".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|_| "Choose a PNG, JPEG, GIF, WebP, or BMP image.".to_owned())?;
    Ok(if image.width() > 2048 || image.height() > 2048 {
        image.thumbnail(2048, 2048)
    } else {
        image
    }
    .into_rgba8())
}

/// Own a validated copy, so moving or deleting the original does not break it.
pub fn import_image(source: &Path, directory: &Path, project: &str) -> Result<String, String> {
    let image = read_image(source)?;
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let mut hash = DefaultHasher::new();
    project.hash(&mut hash);
    image.dimensions().hash(&mut hash);
    image.as_raw().hash(&mut hash);
    let path: PathBuf = directory.join(format!("background-{:016x}.png", hash.finish()));
    if !path.exists() {
        image
            .save(&path)
            .map_err(|e| format!("Cannot save image: {e}"))?;
    }
    Ok(path.to_string_lossy().into_owned())
}

pub fn background_fade(y: f32, empty: bool, haze: bool) -> f32 {
    let stops: &[(f32, f32)] = match (empty, haze) {
        (true, true) => &[
            (0., 1.),
            (0.19, 1.),
            (0.30, 0.94),
            (0.42, 0.72),
            (0.54, 0.42),
            (0.66, 0.16),
            (0.78, 0.04),
            (0.90, 0.),
        ],
        (false, true) => &[(0., 1.), (0.18, 1.), (0.76, 0.)],
        (true, false) => &[
            (0., 1.),
            (0.19, 1.),
            (0.30, 0.79),
            (0.42, 0.50),
            (0.54, 0.20),
            (0.64, 0.05),
            (0.72, 0.),
        ],
        (false, false) => &[
            (0., 1.),
            (0.18, 1.),
            (0.32, 0.61),
            (0.47, 0.32),
            (0.62, 0.11),
            (0.72, 0.),
        ],
    };
    for pair in stops.windows(2) {
        if y <= pair[1].0 {
            let t = ((y - pair[0].0) / (pair[1].0 - pair[0].0)).clamp(0., 1.);
            return pair[0].1 + (pair[1].1 - pair[0].1) * t;
        }
    }
    0.
}

/// View-independent artwork, matching the React worker. Crop, fade and Haze
/// belong to the view and must not trigger another texture job during resize.
pub fn background_texture(source: &RgbaImage, effect: Effect) -> RgbaImage {
    if matches!(effect, Effect::None | Effect::GradientBlur) {
        source.clone()
    } else {
        effect_pixels(source, effect, true, false, 1.)
    }
}

fn effect_pixels(
    source: &RgbaImage,
    effect: Effect,
    empty: bool,
    preview: bool,
    scale: f32,
) -> RgbaImage {
    let (width, height) = source.dimensions();
    let mut output = source.clone();
    let soft = (effect == Effect::GradientBlur).then(|| {
        imageops::blur(
            source,
            if preview {
                2.
            } else if empty {
                7.
            } else {
                5.
            } * scale,
        )
    });
    let strong = (effect == Effect::GradientBlur).then(|| {
        imageops::blur(
            source,
            if preview {
                5.
            } else if empty {
                18.
            } else {
                12.
            } * scale,
        )
    });
    const BAYER: [[f32; 4]; 4] = [
        [0., 8., 2., 10.],
        [12., 4., 14., 6.],
        [3., 11., 1., 9.],
        [15., 7., 13., 5.],
    ];
    const GLYPHS: [[u8; 7]; 10] = [
        [0, 0, 0, 0, 0, 0, 0],
        [0, 0, 0, 0, 0, 4, 0],
        [0, 4, 0, 0, 4, 0, 0],
        [0, 0, 0, 14, 0, 0, 0],
        [0, 0, 14, 0, 14, 0, 0],
        [0, 4, 4, 31, 4, 4, 0],
        [0, 21, 14, 31, 14, 21, 0],
        [10, 10, 31, 10, 31, 10, 10],
        [17, 2, 4, 4, 8, 16, 17],
        [14, 17, 23, 21, 23, 16, 14],
    ];
    let sample = |x: u32, y: u32| source.get_pixel(x.min(width - 1), y.min(height - 1));
    let luma = |p: &image::Rgba<u8>| {
        (p[0] as f32 * 0.2126 + p[1] as f32 * 0.7152 + p[2] as f32 * 0.0722).round()
    };
    for (x, y, pixel) in output.enumerate_pixels_mut() {
        let at = y as f32 / height as f32;
        match effect {
            Effect::None => {}
            Effect::Dither => {
                let p = sample(x / 2 * 2 + 1, y / 2 * 2 + 1);
                let peak = p[0].max(p[1]).max(p[2]) as f32;
                let gain = if peak / 255.
                    > (BAYER[(y / 2 % 4) as usize][(x / 2 % 4) as usize] + 0.5) / 16.
                {
                    255. / peak.max(1.)
                } else {
                    0.08
                };
                for c in 0..3 {
                    pixel[c] = (p[c] as f32 * gain).round() as u8;
                }
                pixel[3] = p[3];
            }
            Effect::Ascii => {
                let p = sample(x / 6 * 6 + 3, y / 8 * 8 + 4);
                let glyph = &GLYPHS[((luma(p) / 255.).sqrt() * 9.) as usize];
                let ink =
                    x % 6 < 5 && y % 8 < 7 && (glyph[(y % 8) as usize] & (1 << (4 - x % 6))) != 0;
                for c in 0..3 {
                    pixel[c] = (pixel[c] as f32 * 0.6 + if ink { p[c] as f32 * 0.4 } else { 0. })
                        .round() as u8;
                }
            }
            Effect::Halftone => {
                let radius = 2. * (0.3 + 0.7 * (luma(sample(x / 4 * 4, y / 4 * 4)) / 255.).sqrt());
                let p = sample(x / 4 * 4 + 2, y / 4 * 4 + 2);
                let distance = (x as f32 % 4. - 1.5).hypot(y as f32 % 4. - 1.5);
                let coverage = (radius + 0.5 - distance).clamp(0., 1.) * p[3] as f32 / 255.;
                for c in 0..3 {
                    pixel[c] = (pixel[c] as f32 * 0.6 + p[c] as f32 * coverage * 0.4).round() as u8;
                }
            }
            Effect::Scanlines => {
                if y % 3 == 0 {
                    for c in 0..3 {
                        pixel[c] = (pixel[c] as f32 * 0.52).round() as u8;
                    }
                }
            }
            Effect::GradientBlur => {
                let soft_mix = ((at - 0.18) / if empty { 0.42 } else { 0.52 }).clamp(0., 1.);
                let strong_mix = ((at - if empty { 0.28 } else { 0.22 })
                    / if empty { 0.48 } else { 0.54 })
                .clamp(0., 1.);
                let darken = if empty {
                    ((at - 0.16) / 0.59).clamp(0., 1.) * 0.78
                } else {
                    ((at - 0.15) / 0.61).clamp(0., 1.) * 0.70
                };
                for c in 0..3 {
                    let a = pixel[c] as f32 * (1. - soft_mix)
                        + soft.as_ref().unwrap().get_pixel(x, y)[c] as f32 * soft_mix;
                    let b = a * (1. - strong_mix)
                        + strong.as_ref().unwrap().get_pixel(x, y)[c] as f32 * strong_mix;
                    let mix = if y % 3 == 0 {
                        1. - (1. - darken) * 0.64
                    } else {
                        darken
                    };
                    pixel[c] = (b * (1. - mix) + 23. * mix).round() as u8;
                }
            }
        }
    }
    output
}

pub(crate) fn png_bytes(image: RgbaImage) -> Result<Vec<u8>, String> {
    let mut bytes = Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(bytes.into_inner())
}

#[cfg(test)]
mod tests;
