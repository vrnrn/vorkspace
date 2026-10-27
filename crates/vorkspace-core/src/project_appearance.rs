//! Project appearance and the image treatments from React's background worker.
use image::{imageops, ImageReader, RgbaImage};
use serde::{Deserialize, Serialize};
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    io::Cursor,
    path::{Path, PathBuf},
};
pub type SourceImage = RgbaImage;

pub const COLORS: [u32; 9] = [
    0x8b949c, 0x459cf7, 0xea603e, 0xf4bf25, 0x39c363, 0xe45a9f, 0xb168d4, 0x37beb4, 0xef8c39,
];
pub const MASCOTS: [&str; 10] = [
    "invader", "ghost", "robot", "cat", "skull", "crab", "mushroom", "rocket", "dino", "frog",
];
pub const BADGE_SHAPES: [&str; 5] = ["squircle", "circle", "rounded", "pill", "hex"];
pub const BOT_FACES: [&str; 5] = ["pills", "dots", "visor", "carets", "wink"];
pub const LOCKED_RINGS: usize = 4;
pub const LOCKED_CURVATURE: f32 = 0.80;
pub const LOCKED_TEX_OPACITY: f32 = 0.60;
pub const SIDEBAR_PILL_OPACITY: f32 = 0.30;

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

/// Map a mascot or face name to one of the 5 3D curved face styles.
pub fn normalize_bot_face(face_or_mascot: &str) -> &'static str {
    match face_or_mascot {
        "pills" | "invader" | "skull" => "pills",
        "dots" | "ghost" | "mushroom" => "dots",
        "visor" | "robot" | "rocket" => "visor",
        "carets" | "cat" | "crab" => "carets",
        "wink" | "dino" | "frog" => "wink",
        _ => "pills",
    }
}

/// Normalize a badge shape name to one of the 5 supported shapes.
pub fn normalize_badge_shape(shape: &str) -> &'static str {
    match shape {
        "squircle" => "squircle",
        "circle" => "circle",
        "rounded" => "rounded",
        "pill" => "pill",
        "hex" => "hex",
        _ => "squircle",
    }
}

fn default_textured() -> bool {
    true
}
fn is_true(value: &bool) -> bool {
    *value
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProjectAppearance {
    pub name: String,
    pub color: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fg_color: Option<u32>,
    #[serde(default = "default_textured", skip_serializing_if = "is_true")]
    pub textured: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid: Option<u8>,
    pub mascot: Option<String>,
    pub logo: Option<String>,
    pub background: Option<Background>,
}
impl Default for ProjectAppearance {
    fn default() -> Self {
        Self {
            name: String::new(),
            color: None,
            fg_color: None,
            textured: true,
            seed: None,
            grid: None,
            mascot: None,
            logo: None,
            background: None,
        }
    }
}
impl ProjectAppearance {
    pub fn label(&self, project: &str) -> String {
        if self.name.trim().is_empty() {
            Path::new(project)
                .file_name()
                .map_or_else(|| project.into(), |s| s.to_string_lossy().into_owned())
        } else {
            self.name.clone()
        }
    }
    pub fn color(&self, project: &str) -> u32 {
        self.color.filter(|c| *c <= 0xffffff).unwrap_or_else(|| {
            let hash = project
                .encode_utf16()
                .fold(0u32, |h, c| h.wrapping_mul(31).wrapping_add(c.into()));
            COLORS[(hash as usize % (COLORS.len() - 1)) + 1]
        })
    }
    pub fn fg_color(&self, project: &str) -> u32 {
        self.fg_color
            .filter(|c| *c <= 0xffffff)
            .unwrap_or_else(|| default_foreground_color(self.color(project)))
    }
    pub fn seed(&self, project: &str) -> u32 {
        self.seed
            .filter(|&s| s > 0)
            .unwrap_or_else(|| default_seed_for(project))
    }
    pub fn grid(&self) -> u8 {
        self.grid.unwrap_or(4).clamp(2, 8)
    }
    pub fn mascot(&self, project: &str) -> usize {
        self.mascot
            .as_ref()
            .and_then(|name| MASCOTS.iter().position(|m| *m == name))
            .unwrap_or_else(|| {
                let hash = project
                    .encode_utf16()
                    .fold(0u32, |h, c| h.wrapping_mul(131).wrapping_add(c.into()));
                hash as usize % MASCOTS.len()
            })
    }
    pub fn normalize(&mut self) {
        self.name = self.name.trim().chars().take(200).collect();
        self.color = self.color.filter(|c| *c <= 0xffffff);
        self.fg_color = self.fg_color.filter(|c| *c <= 0xffffff);
        self.seed = self.seed.filter(|&s| s > 0);
        self.grid = self.grid.map(|g| g.clamp(2, 8));
        self.mascot = self.mascot.take().filter(|s| MASCOTS.contains(&s.as_str()));
        if let Some(background) = &mut self.background {
            background.normalize();
        }
        self.background = self.background.take().filter(|b| !b.path.trim().is_empty());
    }
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
pub fn import_image(
    source: &Path,
    directory: &Path,
    project: &str,
    logo: bool,
) -> Result<String, String> {
    let image = read_image(source)?;
    let image = if logo && (image.width() > 256 || image.height() > 256) {
        imageops::thumbnail(&image, 256, 256)
    } else {
        image
    };
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let mut hash = DefaultHasher::new();
    project.hash(&mut hash);
    image.dimensions().hash(&mut hash);
    image.as_raw().hash(&mut hash);
    let path: PathBuf = directory.join(format!(
        "{}-{:016x}.png",
        if logo { "logo" } else { "background" },
        hash.finish()
    ));
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

/// Backends without the Metal layer compositor bake Haze into the source;
/// their cover crop and opacity mask still follow the current view immediately.
pub fn software_background_texture(
    source: &RgbaImage,
    effect: Effect,
    empty: bool,
    preview: bool,
) -> RgbaImage {
    if effect == Effect::GradientBlur {
        effect_pixels(source, effect, empty, preview, 1.)
    } else {
        background_texture(source, effect)
    }
}

/// React processes texture effects at the artwork's resolution, then the view
/// cover-crops the result and applies its own fade. CSS blur uses view pixels.
pub fn render_background(
    source: &RgbaImage,
    effect: Effect,
    width: u32,
    height: u32,
    empty: bool,
) -> RgbaImage {
    render_background_scaled(source, effect, width, height, empty, false, 1.)
}
pub fn render_background_scaled(
    source: &RgbaImage,
    effect: Effect,
    width: u32,
    height: u32,
    empty: bool,
    preview: bool,
    scale: f32,
) -> RgbaImage {
    let textured = if matches!(
        effect,
        Effect::Dither | Effect::Ascii | Effect::Halftone | Effect::Scanlines
    ) {
        effect_pixels(source, effect, empty, preview, scale)
    } else {
        source.clone()
    };
    let covered = cover_image(&textured, width, height);
    let mut output = if effect == Effect::GradientBlur {
        effect_pixels(&covered, effect, empty, preview, scale)
    } else {
        covered
    };
    let height = output.height();
    for (_, y, pixel) in output.enumerate_pixels_mut() {
        pixel[3] = (pixel[3] as f32
            * background_fade(
                y as f32 / height as f32,
                empty,
                effect == Effect::GradientBlur,
            ))
        .round() as u8;
    }
    output
}
fn cover_image(source: &RgbaImage, width: u32, height: u32) -> RgbaImage {
    let width = width.clamp(1, 2048);
    let height = height.clamp(1, 2048);
    let aspect = width as f64 / height as f64;
    let crop_width = source
        .width()
        .min((source.height() as f64 * aspect).round().max(1.) as u32);
    let crop_height = source
        .height()
        .min((source.width() as f64 / aspect).round().max(1.) as u32);
    let cropped = imageops::crop_imm(
        source,
        (source.width() - crop_width) / 2,
        (source.height() - crop_height) / 2,
        crop_width,
        crop_height,
    )
    .to_image();
    imageops::resize(&cropped, width, height, imageops::FilterType::Triangle)
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

pub fn png_bytes(image: RgbaImage) -> Result<Vec<u8>, String> {
    let mut bytes = Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(bytes.into_inner())
}

// ============================================================================
// Concentric Texture Engine & 3D Curved Face / Badge / Pill Renderer
// ============================================================================

struct Arc4 {
    i: u8,
    j: u8,
    s: [u8; 256],
}
impl Arc4 {
    fn new(seed: &str) -> Self {
        let mut key = Vec::new();
        for (idx, ch) in seed.encode_utf16().enumerate() {
            let k_idx = idx & 0xff;
            if k_idx >= key.len() {
                key.push(0u8);
            }
            let prev = key[k_idx];
            let next = ((prev as u32).wrapping_mul(19) + ch as u32) as u8;
            key[k_idx] = next;
        }
        if key.is_empty() {
            key.push(0);
        }
        let mut s = [0u8; 256];
        for (idx, slot) in s.iter_mut().enumerate() {
            *slot = idx as u8;
        }
        let mut m = 0u8;
        for j in 0..256 {
            let p = s[j];
            m = m.wrapping_add(key[j % key.len()]).wrapping_add(p);
            s[j] = s[m as usize];
            s[m as usize] = p;
        }
        let mut arc4 = Self { i: 0, j: 0, s };
        for _ in 0..256 {
            arc4.next_byte();
        }
        arc4
    }
    fn next_byte(&mut self) -> u8 {
        self.i = self.i.wrapping_add(1);
        let z = self.s[self.i as usize];
        self.j = self.j.wrapping_add(z);
        let sz = self.s[self.j as usize];
        self.s[self.i as usize] = sz;
        self.s[self.j as usize] = z;
        self.s[(z.wrapping_add(sz)) as usize]
    }
    fn g(&mut self, count: usize) -> f64 {
        let mut f = 0.0f64;
        for _ in 0..count {
            f = f * 256.0 + self.next_byte() as f64;
        }
        f
    }
    fn next_f64(&mut self) -> f64 {
        let u = 256.0f64;
        let l = 6usize;
        let c = u.powi(l as i32);
        let d = (1u64 << 52) as f64;
        let m = d * 2.0;
        let mut nt = self.g(l);
        let mut z = c;
        let mut f = 0u32;
        while nt < d {
            nt = (nt + f as f64) * u;
            z *= u;
            f = self.next_byte() as u32;
        }
        while nt >= m {
            nt /= 2.0;
            z /= 2.0;
            f >>= 1;
        }
        (nt + f as f64) / z
    }
    fn rnd_int(&mut self, min: i32, max: i32) -> i32 {
        (self.next_f64() * (max - min) as f64).floor() as i32 + min
    }
}

#[derive(Clone, Copy, Debug)]
pub struct WalkStep {
    pub s: usize,
    pub c: usize,
    pub corner: u8, // 0 = (0,0), 1 = (c,0), 2 = (c,c), 3 = (0,c)
}

#[derive(Clone, Debug)]
pub struct ConcentricGeometry {
    pub default_rotation_deg: f32,
    pub steps: Vec<WalkStep>,
    pub grid_dim: usize,
}

pub fn compute_concentric_geometry(seed: u32, grid: u8) -> ConcentricGeometry {
    let l = grid.clamp(2, 8) as usize;
    let mut rng = Arc4::new(&seed.to_string());
    let default_rotation_deg = (rng.rnd_int(0, 4) * 15) as f32;
    const XL: [isize; 4] = [0, 1, 0, -1];
    const YL: [isize; 4] = [-1, 0, 1, 0];

    let mut steps = Vec::new();
    let mut visited = vec![vec![false; l]; l];
    let mut k = 0usize;
    let mut s = 0isize;
    let mut c_cell = 0isize;
    let mut t = if rng.next_f64() > 0.5 { 1usize } else { 2usize };
    let d = l * l;
    let max_steps = d * 45;

    let push_step = |steps: &mut Vec<WalkStep>, s: usize, c: usize, p: usize, v: usize| {
        let b = (p + 2) % 4;
        let j = v;
        let (a0, a1) = if b <= j { (b, j) } else { (j, b) };
        let corner = match (a0, a1) {
            (0, 3) => 0,
            (0, 1) => 1,
            (1, 2) => 2,
            (2, 3) => 3,
            _ => return,
        };
        steps.push(WalkStep { s, c, corner });
    };

    for _ in 0..max_steps {
        let r_done = k >= d;
        if !visited[s as usize][c_cell as usize] {
            visited[s as usize][c_cell as usize] = true;
            k += 1;
        }
        let a = (t + 3) % 4;
        let p = (t + 1) % 4;
        let mut choices = Vec::with_capacity(2);
        let b = s + XL[a];
        let j = c_cell + YL[a];
        if b >= 0 && b < l as isize && j >= 0 && j < l as isize {
            choices.push(a);
        }
        let g = s + XL[p];
        let nt = c_cell + YL[p];
        let z = g >= 0 && g < l as isize && nt >= 0 && nt < l as isize;
        if z {
            choices.push(p);
        }
        if r_done && (s == 0 || s == l as isize - 1 || c_cell == 0 || c_cell == l as isize - 1) {
            push_step(
                &mut steps,
                s as usize,
                c_cell as usize,
                t,
                if z { a } else { p },
            );
            break;
        }
        if choices.is_empty() {
            break;
        }
        let f = choices[rng.rnd_int(0, choices.len() as i32) as usize];
        push_step(&mut steps, s as usize, c_cell as usize, t, f);
        s += XL[f];
        c_cell += YL[f];
        t = f;
    }

    ConcentricGeometry {
        default_rotation_deg,
        steps,
        grid_dim: l,
    }
}

#[inline]
fn unpack_rgb(hex: u32) -> [f32; 3] {
    [
        ((hex >> 16) & 0xff) as f32,
        ((hex >> 8) & 0xff) as f32,
        (hex & 0xff) as f32,
    ]
}

#[inline]
fn lerp_rgb(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

// Pose changes do not change the underlying texture. Keep exact rotation bits:
// snapping here would change the public renderer's pixels, including previews.
type TextureKey = ((u32, u8, bool, u32, u32, u32), u32);
const TEXTURE_CACHE_BYTES: usize = 9 * 1024 * 1024;

#[derive(Default)]
struct TextureCache {
    // Least recently used first. The byte budget holds at most 64 192px images.
    entries: std::collections::VecDeque<(TextureKey, std::sync::Arc<RgbaImage>)>,
    bytes: usize,
}

impl TextureCache {
    fn get(&mut self, key: &TextureKey) -> Option<std::sync::Arc<RgbaImage>> {
        let at = self
            .entries
            .iter()
            .position(|(candidate, _)| candidate == key)?;
        let entry = self.entries.remove(at)?;
        let image = entry.1.clone();
        self.entries.push_back(entry);
        Some(image)
    }

    fn insert(&mut self, key: TextureKey, image: std::sync::Arc<RgbaImage>) {
        let bytes = image.as_raw().len();
        // Oversized one-off images must not evict the hot working set.
        if bytes > TEXTURE_CACHE_BYTES {
            return;
        }
        if let Some(at) = self
            .entries
            .iter()
            // Only the latest exact rotation of a look is useful to animation.
            // Spinning one Bot must not evict every other look's warm texture.
            .position(|(candidate, _)| candidate.0 == key.0)
        {
            self.bytes -= self.entries.remove(at).unwrap().1.as_raw().len();
        }
        while self.bytes + bytes > TEXTURE_CACHE_BYTES {
            self.bytes -= self.entries.pop_front().unwrap().1.as_raw().len();
        }
        self.bytes += bytes;
        self.entries.push_back((key, image));
    }
}

fn cached_concentric_texture(
    seed: u32,
    grid: u8,
    textured: bool,
    bg_hex: u32,
    fg_hex: u32,
    bucket: u32,
    rot_offset_deg: f32,
) -> std::sync::Arc<RgbaImage> {
    use std::sync::{Arc, LazyLock, Mutex};
    static CACHE: LazyLock<Mutex<TextureCache>> =
        LazyLock::new(|| Mutex::new(TextureCache::default()));
    let key = (
        (seed, grid, textured, bg_hex, fg_hex, bucket),
        rot_offset_deg.to_bits(),
    );
    if let Some(texture) = CACHE.lock().ok().and_then(|mut cache| cache.get(&key)) {
        return texture;
    }
    // Rasterization never holds the shared lock. Concurrent misses may duplicate
    // computation but cannot block other lookups for a full rasterization.
    let texture = Arc::new(render_concentric_texture(
        seed,
        grid,
        textured,
        bg_hex,
        fg_hex,
        bucket,
        rot_offset_deg,
    ));
    if let Ok(mut cache) = CACHE.lock() {
        cache.insert(key, texture.clone());
    }
    texture
}

/// Rasterize the concentric ring texture at `bucket x bucket` resolution.
pub fn render_concentric_texture(
    seed: u32,
    grid: u8,
    textured: bool,
    bg_hex: u32,
    fg_hex: u32,
    bucket: u32,
    rot_offset_deg: f32,
) -> RgbaImage {
    let bg = unpack_rgb(bg_hex);
    if !textured {
        return RgbaImage::from_pixel(
            bucket,
            bucket,
            image::Rgba([bg[0] as u8, bg[1] as u8, bg[2] as u8, 255]),
        );
    }
    let fg = unpack_rgb(fg_hex);
    let geo = compute_concentric_geometry(seed, grid);
    let l = geo.grid_dim;
    let c = 1.0f32 / l as f32;
    let p = LOCKED_RINGS;
    let f = 0.72f32;
    let base_w = c / (p as f32 * 2.0 - 1.0);
    let half_stroke = base_w * f * 0.5;
    let mut radii = [0.0f32; LOCKED_RINGS];
    for (q, r) in radii.iter_mut().enumerate() {
        let at = q as f32 + 0.5 - p as f32 / 2.0;
        *r = c / 2.0 + at * (c / (p as f32 - 0.5) * (0.5 + f * 0.5));
    }

    // Group steps by cell in painter's order so each pixel only evaluates the
    // steps that touched its cell `(gx, gy)`.
    let mut cell_steps: Vec<Vec<u8>> = vec![Vec::new(); l * l];
    for step in &geo.steps {
        if step.s < l && step.c < l {
            let cell = &mut cell_steps[step.c * l + step.s];
            cell.push(step.corner);
        }
    }

    let m = (l as f32 / (l.saturating_sub(1).max(1) as f32)).max(1.42);
    let angle = -(geo.default_rotation_deg + rot_offset_deg).to_radians();
    let cos_a = angle.cos() / m;
    let sin_a = angle.sin() / m;
    let inv_bucket = 1.0f32 / bucket.max(1) as f32;
    let aa = inv_bucket / m * 1.15;

    let mut img = RgbaImage::new(bucket, bucket);
    for py in 0..bucket {
        let ny = (py as f32 + 0.5) * inv_bucket - 0.5;
        for px in 0..bucket {
            let nx = (px as f32 + 0.5) * inv_bucket - 0.5;
            let u = nx * cos_a - ny * sin_a + 0.5;
            let v = nx * sin_a + ny * cos_a + 0.5;

            let mut off_col = bg;
            if (0.0..1.0).contains(&u) && (0.0..1.0).contains(&v) {
                let gx = ((u * l as f32).floor() as usize).min(l - 1);
                let gy = ((v * l as f32).floor() as usize).min(l - 1);
                let f_x = gx as f32 * c;
                let b_y = gy as f32 * c;
                for &corner in &cell_steps[gy * l + gx] {
                    let (mx, gy_c) = match corner {
                        0 => (0.0, 0.0),
                        1 => (c, 0.0),
                        2 => (c, c),
                        _ => (0.0, c),
                    };
                    let dist = (u - (f_x + mx)).hypot(v - (b_y + gy_c));
                    // Quarter-circle background mask that occludes prior strokes in this cell
                    let mask_alpha = ((c - dist) / aa + 0.5).clamp(0.0, 1.0);
                    if mask_alpha > 0.0 {
                        off_col = lerp_rgb(off_col, bg, mask_alpha);
                    }
                    // Concentric ring strokes
                    let mut min_ring_dist = f32::MAX;
                    for &r in &radii {
                        let d = (dist - r).abs();
                        if d < min_ring_dist {
                            min_ring_dist = d;
                        }
                    }
                    let stroke_alpha = ((half_stroke - min_ring_dist) / aa + 0.5).clamp(0.0, 1.0);
                    if stroke_alpha > 0.0 {
                        off_col = lerp_rgb(off_col, fg, stroke_alpha);
                    }
                }
            }
            let out = lerp_rgb(bg, off_col, LOCKED_TEX_OPACITY);
            img.put_pixel(
                px,
                py,
                image::Rgba([
                    out[0].round() as u8,
                    out[1].round() as u8,
                    out[2].round() as u8,
                    255,
                ]),
            );
        }
    }
    img
}

/// Render a wide rectangular textured pill for selected Project or Channel rows
/// in the sidebar (`30%` overall opacity).
pub fn render_textured_pill(
    seed: u32,
    grid: u8,
    textured: bool,
    bg_hex: u32,
    fg_hex: u32,
    width: u32,
    height: u32,
) -> RgbaImage {
    let w = width.max(16);
    let h = height.max(8);
    let tex = render_concentric_texture(seed, grid, textured, bg_hex, fg_hex, 240, 0.0);
    let tw = tex.width() as f32;
    let th = tex.height() as f32;
    let sh = (tw * (h as f32 / w as f32)).min(th);
    let sy = (th - sh) * 0.5;

    let mut out = RgbaImage::new(w, h);
    let alpha_u8 = (SIDEBAR_PILL_OPACITY * 255.0).round() as u8;
    for y in 0..h {
        let ty = ((sy + (y as f32 + 0.5) / h as f32 * sh) as u32).min(tex.height() - 1);
        for x in 0..w {
            let tx = (((x as f32 + 0.5) / w as f32 * tw) as u32).min(tex.width() - 1);
            let p = tex.get_pixel(tx, ty).0;
            let mut col = [p[0] as f32, p[1] as f32, p[2] as f32];

            // Subtle dark gradient from left so label text is always crisp
            let u = x as f32 / w.max(1) as f32;
            let dark_alpha = if u < 0.5 {
                0.22 + (0.08 - 0.22) * (u * 2.0)
            } else {
                0.08 + (0.18 - 0.08) * ((u - 0.5) * 2.0)
            };
            col = lerp_rgb(col, [0.0, 0.0, 0.0], dark_alpha);

            // Crisp inner highlight border
            if x <= 1 || x >= w - 2 || y <= 1 || y >= h - 2 {
                col = lerp_rgb(col, [255.0, 255.0, 255.0], 0.16);
            }

            out.put_pixel(
                x,
                y,
                image::Rgba([
                    col[0].round() as u8,
                    col[1].round() as u8,
                    col[2].round() as u8,
                    alpha_u8,
                ]),
            );
        }
    }
    out
}

#[inline]
fn sd_rounded_box(px: f32, py: f32, bx: f32, by: f32, r: f32) -> f32 {
    let qx = px.abs() - bx + r;
    let qy = py.abs() - by + r;
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

#[inline]
fn sd_segment(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let pax = px - ax;
    let pay = py - ay;
    let bax = bx - ax;
    let bay = by - ay;
    let h = ((pax * bax + pay * bay) / (bax * bax + bay * bay).max(1e-6)).clamp(0.0, 1.0);
    (pax - bax * h).hypot(pay - bay * h)
}

/// Signed distance to one of the 5 badge shapes centered at `(s/2, s/2)`.
#[inline]
fn sd_badge_shape(shape: &str, x: f32, y: f32, s: f32) -> f32 {
    let cx = x - s * 0.5;
    let cy = y - s * 0.5;
    match shape {
        "circle" => cx.hypot(cy) - s * 0.47,
        "rounded" => {
            let half = s * 0.46;
            sd_rounded_box(cx, cy, half, half, s * 0.22)
        }
        "pill" => {
            let bx = s * 0.47;
            let by = s * 0.39;
            sd_rounded_box(cx, cy, bx, by, by)
        }
        "hex" => {
            // Pointy-top rounded hexagon matching `r = s * 0.48`, `cr = s * 0.09`
            let cr = s * 0.09;
            let r_in = s * 0.48 * 0.866_025_4 - cr;
            let mut qx = cx.abs();
            let mut qy = cy.abs();
            // Pointy-top hex symmetry: normals at (1, 0) and (0.5, sqrt(3)/2)
            let dot = 0.5 * qx + 0.866_025_4 * qy;
            if dot > 0.0 {
                let d = (qx - dot).max(0.0);
                let _ = d;
            }
            // Exact pointy-top hex SDF via 60-degree folding
            // Rotate by 30 deg so flat-top hex formula applies
            let rx = (cx * 0.866_025_4 - cy * 0.5).abs();
            let ry = (cx * 0.5 + cy * 0.866_025_4).abs();
            qx = rx;
            qy = ry;
            const K_X: f32 = -0.866_025_4;
            const K_Y: f32 = 0.5;
            const K_Z: f32 = 0.577_350_26;
            let m2 = 2.0 * (K_X * qx + K_Y * qy).min(0.0);
            qx -= m2 * K_X;
            qy -= m2 * K_Y;
            qx -= qx.clamp(-K_Z * r_in, K_Z * r_in);
            qy -= r_in;
            qx.hypot(qy) * qy.signum() - cr
        }
        _ => {
            // "squircle"
            let half = s * 0.46;
            sd_rounded_box(cx, cy, half, half, s * 0.30)
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BotAnimState {
    pub yaw: f32,
    pub pitch: f32,
    pub blink: f32,
    pub squash_x: f32,
    pub squash_y: f32,
    pub hop_y: f32,
    pub wide: f32,
    pub tex_rot: f32,
    /// Transient expression for destructive-action hover; never saved as a face style.
    pub crossed_eyes: bool,
}
impl Default for BotAnimState {
    fn default() -> Self {
        Self {
            yaw: 0.0,
            pitch: 0.0,
            blink: 1.0,
            squash_x: 1.0,
            squash_y: 1.0,
            hop_y: 0.0,
            wide: 1.0,
            tex_rot: 0.0,
            crossed_eyes: false,
        }
    }
}

#[derive(Clone, Copy)]
struct ProjectedEye {
    cx: f32,
    cy: f32,
    cos_r: f32,
    sin_r: f32,
    inv_sx: f32,
    inv_sy: f32,
    scale_avg: f32,
}

fn project_eye(u: f32, v: f32, yaw: f32, pitch: f32, s: f32, wide: f32) -> ProjectedEye {
    let k = LOCKED_CURVATURE;
    let r2 = (u * u + v * v).min(0.85);
    let z0 = (1.0 - r2).sqrt();

    let (sy, cy) = (yaw * k).sin_cos();
    let (sp, cp) = (pitch * k).sin_cos();

    let x1 = u * cy + z0 * sy;
    let z1 = -u * sy + z0 * cy;
    let y2 = v * cp + z1 * sp;
    let z2 = -v * sp + z1 * cp;

    let foreshorten = z2.max(0.35);
    let rot = (-yaw * pitch * 0.95 - x1 * y2 * 0.55) * k;
    let sx = (1.0 - (x1.abs() * 0.58 * k).min(0.48)) * wide;
    let sy_scale = (1.0 - (1.0 - foreshorten) * 0.22 * k) * wide;
    let (sin_r, cos_r) = rot.sin_cos();
    let radius = s * 0.43;
    ProjectedEye {
        cx: s * 0.5 + x1 * radius,
        cy: s * 0.5 + y2 * radius,
        cos_r,
        sin_r,
        inv_sx: 1.0 / sx.max(0.1),
        inv_sy: 1.0 / sy_scale.max(0.1),
        scale_avg: (sx + sy_scale) * 0.5,
    }
}

impl ProjectedEye {
    #[inline]
    fn to_local(self, px: f32, py: f32) -> (f32, f32) {
        let dx = px - self.cx;
        let dy = py - self.cy;
        let rx = dx * self.cos_r + dy * self.sin_r;
        let ry = -dx * self.sin_r + dy * self.cos_r;
        (rx * self.inv_sx, ry * self.inv_sy)
    }
}

/// Render a Project badge (`is_bot = false`, always Soft Hex) or a 3D Bot avatar
/// (`is_bot = true`, with 3D curved face & optional `needs-you` notification dot).
#[allow(clippy::too_many_arguments)]
pub fn render_badge_or_bot(
    is_bot: bool,
    shape: &str,
    face: &str,
    needs_you: bool,
    textured: bool,
    seed: u32,
    grid: u8,
    bg_hex: u32,
    fg_hex: u32,
    pixel_side: u32,
    anim: BotAnimState,
) -> RgbaImage {
    let s_u32 = pixel_side.max(16);
    let s = s_u32 as f32;
    let bucket = if s_u32 <= 64 { 96 } else { 192 };
    let tex = cached_concentric_texture(
        seed,
        grid,
        textured,
        bg_hex,
        fg_hex,
        bucket,
        if is_bot { anim.tex_rot } else { 0.0 },
    );
    let shape_id = if is_bot {
        normalize_badge_shape(shape)
    } else {
        "hex"
    };
    let face_id = normalize_bot_face(face);
    let bg_rgb = unpack_rgb(bg_hex);
    let face_rgb = unpack_rgb(if textured {
        fg_hex
    } else {
        auto_contrast_color(bg_hex)
    });

    let eye_left = project_eye(
        -0.27,
        if face_id == "pills" { -0.02 } else { -0.01 },
        anim.yaw,
        anim.pitch,
        s,
        anim.wide,
    );
    let eye_right = project_eye(
        0.27,
        if face_id == "pills" { -0.02 } else { -0.01 },
        anim.yaw,
        anim.pitch,
        s,
        anim.wide,
    );
    let eye_center = project_eye(0.0, -0.01, anim.yaw, anim.pitch, s, anim.wide);

    let hx = s * (0.5 + anim.yaw * 0.22);
    let hy = s * (0.38 + anim.pitch * 0.22);
    let r_inner = s * 0.04;
    let r_outer = s * 0.62;
    let border_w = (s * 0.025).max(1.0);

    let mut out = RgbaImage::new(s_u32, s_u32);
    let pivot_x = s * 0.5;
    let pivot_y = s * 0.88;
    let hop_px = anim.hop_y * s;
    let inv_squash_x = 1.0 / anim.squash_x.max(0.2);
    let inv_squash_y = 1.0 / anim.squash_y.max(0.2);

    for py in 0..s_u32 {
        let fy = py as f32 + 0.5;
        for px in 0..s_u32 {
            let fx = px as f32 + 0.5;

            // Inverse squash & hop transform
            let sx = (fx - pivot_x) * inv_squash_x + pivot_x;
            let sy = (fy - (pivot_y + hop_px)) * inv_squash_y + pivot_y;

            let d_shape = sd_badge_shape(shape_id, sx, sy, s);
            let shape_alpha = (0.5 - d_shape).clamp(0.0, 1.0);
            let mut rgba = [0.0f32; 4];

            if shape_alpha > 0.0 {
                let tx = (((sx / s).clamp(0.0, 0.9999) * bucket as f32) as u32).min(bucket - 1);
                let ty = (((sy / s).clamp(0.0, 0.9999) * bucket as f32) as u32).min(bucket - 1);
                let tp = tex.get_pixel(tx, ty).0;
                let mut col = [tp[0] as f32, tp[1] as f32, tp[2] as f32];

                // 3D spherical radial shading
                let d_hi = (sx - hx).hypot(sy - hy);
                let t_rad = ((d_hi - r_inner) / (r_outer - r_inner).max(1.0)).clamp(0.0, 1.0);
                if t_rad < 0.55 {
                    let hi = (1.0 - t_rad / 0.55) * 0.12;
                    col = lerp_rgb(col, [255.0, 255.0, 255.0], hi);
                } else {
                    let sh = ((t_rad - 0.55) / 0.45) * 0.22;
                    col = lerp_rgb(col, [0.0, 0.0, 0.0], sh);
                }

                // 3D curved face (Bots only)
                if is_bot {
                    match if anim.crossed_eyes {
                        "crossed"
                    } else {
                        face_id
                    } {
                        "crossed" => {
                            let r = s * 0.085;
                            let half_w = s * 0.025;
                            for eye in [eye_left, eye_right] {
                                let (lx, ly) = eye.to_local(sx, sy);
                                let d1 = sd_segment(lx, ly, -r, -r, r, r);
                                let d2 = sd_segment(lx, ly, -r, r, r, -r);
                                let d = (d1.min(d2) - half_w) * eye.scale_avg;
                                let a = (0.5 - d).clamp(0.0, 1.0);
                                col = lerp_rgb(col, face_rgb, a);
                            }
                        }
                        "pills" => {
                            let ew = s * 0.118 * 0.5;
                            let eh = (s * 0.26 * anim.blink).max(s * 0.118 * 0.35) * 0.5;
                            for eye in [eye_left, eye_right] {
                                let (lx, ly) = eye.to_local(sx, sy);
                                let d = sd_rounded_box(lx, ly, ew, eh, ew.min(eh)) * eye.scale_avg;
                                let a = (0.5 - d).clamp(0.0, 1.0);
                                if a > 0.0 {
                                    col = lerp_rgb(col, face_rgb, a);
                                }
                            }
                        }
                        "dots" => {
                            let er = s * 0.088;
                            let b = anim.blink.max(0.08);
                            for eye in [eye_left, eye_right] {
                                let (lx, ly) = eye.to_local(sx, sy);
                                let d = (lx.hypot(ly / b) - er) * eye.scale_avg * b.min(1.0);
                                let a = (0.5 - d).clamp(0.0, 1.0);
                                if a > 0.0 {
                                    col = lerp_rgb(col, face_rgb, a);
                                }
                            }
                        }
                        "visor" => {
                            let (lx, ly) = eye_center.to_local(sx, sy);
                            let vw = s * 0.58 * 0.5;
                            let vh = s * 0.25 * 0.5;
                            let d_visor =
                                sd_rounded_box(lx, ly, vw, vh, vh * 0.96) * eye_center.scale_avg;
                            let a_visor = (0.5 - d_visor).clamp(0.0, 1.0);
                            if a_visor > 0.0 {
                                let mut v_col = face_rgb;
                                let er = s * 0.055;
                                let b = anim.blink.max(0.08);
                                for side in [-1.0f32, 1.0f32] {
                                    let ex = lx - side * s * 0.13;
                                    let ey = ly / b;
                                    let d_inner = sd_rounded_box(ex, ey, er, er * 1.15, er * 0.6)
                                        * eye_center.scale_avg
                                        * b.min(1.0);
                                    let a_inner = (0.5 - d_inner).clamp(0.0, 1.0);
                                    if a_inner > 0.0 {
                                        v_col = lerp_rgb(v_col, bg_rgb, a_inner);
                                    }
                                }
                                col = lerp_rgb(col, v_col, a_visor);
                            }
                        }
                        "carets" => {
                            let r = s * 0.095;
                            let half_w = (s * 0.055).max(1.8) * 0.5;
                            let b = anim.blink.max(0.08);
                            for eye in [eye_left, eye_right] {
                                let (lx, ly) = eye.to_local(sx, sy);
                                let ly = ly / b;
                                let d1 = sd_segment(lx, ly, -r * 0.85, r * 0.45, 0.0, -r * 0.65);
                                let d2 = sd_segment(lx, ly, 0.0, -r * 0.65, r * 0.85, r * 0.45);
                                let d = (d1.min(d2) - half_w) * eye.scale_avg * b.min(1.0);
                                let a = (0.5 - d).clamp(0.0, 1.0);
                                if a > 0.0 {
                                    col = lerp_rgb(col, face_rgb, a);
                                }
                            }
                        }
                        "wink" => {
                            // Left eye: pill
                            let ew = s * 0.115 * 0.5;
                            let eh = (s * 0.25 * anim.blink).max(s * 0.115 * 0.35) * 0.5;
                            let (lx, ly) = eye_left.to_local(sx, sy);
                            let d_left =
                                sd_rounded_box(lx, ly, ew, eh, ew.min(eh)) * eye_left.scale_avg;
                            let a_left = (0.5 - d_left).clamp(0.0, 1.0);
                            if a_left > 0.0 {
                                col = lerp_rgb(col, face_rgb, a_left);
                            }
                            // Right eye: '>' caret
                            let r = s * 0.095;
                            let half_w = (s * 0.055).max(1.8) * 0.5;
                            let (rx, ry) = eye_right.to_local(sx, sy);
                            let d1 = sd_segment(rx, ry, -r * 0.7, -r * 0.55, r * 0.55, 0.0);
                            let d2 = sd_segment(rx, ry, r * 0.55, 0.0, -r * 0.7, r * 0.55);
                            let d_right = (d1.min(d2) - half_w) * eye_right.scale_avg;
                            let a_right = (0.5 - d_right).clamp(0.0, 1.0);
                            if a_right > 0.0 {
                                col = lerp_rgb(col, face_rgb, a_right);
                            }
                        }
                        _ => {}
                    }
                }

                // Subtle inner rim highlight
                let d_border = (d_shape + border_w * 0.5).abs() - border_w * 0.5;
                let border_a = (0.5 - d_border).clamp(0.0, 1.0) * 0.14;
                if border_a > 0.0 {
                    col = lerp_rgb(col, [255.0, 255.0, 255.0], border_a);
                }

                rgba = [col[0], col[1], col[2], shape_alpha * 255.0];
            }

            // Notification dot for `needs-you`
            if is_bot && needs_you {
                let br = (s * 0.13).max(2.5);
                let bx = s - br * 1.05;
                let by = br * 1.05;
                let d_dot = (fx - bx).hypot(fy - by) - br;
                let dot_stroke = (s * 0.03).max(1.0);
                let a_outer = (0.5 - (d_dot - dot_stroke * 0.5)).clamp(0.0, 1.0);
                if a_outer > 0.0 {
                    let a_inner = (0.5 - (d_dot + dot_stroke * 0.5)).clamp(0.0, 1.0);
                    let dot_col = lerp_rgb([20.0, 20.0, 22.0], [255.0, 210.0, 74.0], a_inner);
                    let src_a = a_outer;
                    let dst_a = rgba[3] / 255.0;
                    let out_a = src_a + dst_a * (1.0 - src_a);
                    if out_a > 0.0 {
                        rgba[0] = (dot_col[0] * src_a + rgba[0] * dst_a * (1.0 - src_a)) / out_a;
                        rgba[1] = (dot_col[1] * src_a + rgba[1] * dst_a * (1.0 - src_a)) / out_a;
                        rgba[2] = (dot_col[2] * src_a + rgba[2] * dst_a * (1.0 - src_a)) / out_a;
                        rgba[3] = out_a * 255.0;
                    }
                }
            }

            out.put_pixel(
                px,
                py,
                image::Rgba([
                    rgba[0].round() as u8,
                    rgba[1].round() as u8,
                    rgba[2].round() as u8,
                    rgba[3].round() as u8,
                ]),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn texture_cache_preserves_exact_pixels_across_look_and_rotation_changes() {
        use std::sync::Arc;
        let texture = |seed, grid, textured, bg, fg, bucket, rotation| {
            cached_concentric_texture(seed, grid, textured, bg, fg, bucket, rotation)
        };
        let resting = texture(7, 3, true, 0x223344, 0xddeeff, 96, 0.);
        assert!(Arc::ptr_eq(
            &resting,
            &texture(7, 3, true, 0x223344, 0xddeeff, 96, 0.)
        ));
        for (seed, grid, textured, bg, fg, bucket, rotation) in [
            (7, 3, true, 0x223344, 0xddeeff, 96, 10.4),
            (7, 3, true, 0x223344, 0xddeeff, 96, 9.6),
            (8, 8, true, 0x223344, 0xddeeff, 192, -1.25),
            (7, 2, false, 0x112233, 0x445566, 96, 360.),
            (7, 3, true, 0x223344, 0xddeeff, 96, 0.),
        ] {
            assert_eq!(
                *texture(seed, grid, textured, bg, fg, bucket, rotation),
                render_concentric_texture(seed, grid, textured, bg, fg, bucket, rotation)
            );
        }
    }

    #[test]
    fn texture_cache_evicts_by_bytes_without_flushing_hot_entries() {
        use std::sync::Arc;
        let mut cache = TextureCache::default();
        let image = Arc::new(RgbaImage::new(192, 192));
        let key = |seed| ((seed, 3, true, 0, 0, 192), 0);
        for seed in 0..64 {
            cache.insert(key(seed), image.clone());
        }
        assert_eq!(cache.bytes, TEXTURE_CACHE_BYTES);
        assert!(cache.get(&key(0)).is_some());
        cache.insert(key(64), image.clone());
        assert!(cache.get(&key(1)).is_none());
        assert!(cache.get(&key(0)).is_some());
        assert_eq!(cache.entries.len(), 64);
        cache.insert(key(0), image.clone());
        assert_eq!(cache.bytes, TEXTURE_CACHE_BYTES);
        for angle in 1..180 {
            cache.insert((key(0).0, angle), image.clone());
        }
        assert_eq!(cache.bytes, TEXTURE_CACHE_BYTES);
        assert_eq!(cache.entries.len(), 64);
        assert!(cache.get(&(key(0).0, 179)).is_some());
        assert!(cache.get(&key(2)).is_some());
        cache.insert(key(65), Arc::new(RgbaImage::new(2048, 2048)));
        assert_eq!(cache.bytes, TEXTURE_CACHE_BYTES);
        assert!(cache.get(&(key(0).0, 179)).is_some());
    }

    #[test]
    #[ignore = "explicit texture CPU benchmark"]
    fn texture_cache_benchmark() {
        use std::{hint::black_box, time::Instant};
        for bucket in [96, 192] {
            for _ in 0..5 {
                let start = Instant::now();
                for _ in 0..60 {
                    black_box(render_concentric_texture(
                        7, 4, true, 0x223344, 0xddeeff, bucket, 0.,
                    ));
                }
                let baseline = start.elapsed();
                black_box(cached_concentric_texture(
                    7, 4, true, 0x223344, 0xddeeff, bucket, 0.,
                ));
                let start = Instant::now();
                for _ in 0..60 {
                    black_box(cached_concentric_texture(
                        7, 4, true, 0x223344, 0xddeeff, bucket, 0.,
                    ));
                }
                eprintln!(
                    "texture bucket={bucket} samples=60 baseline_us={} cached_us={}",
                    baseline.as_micros(),
                    start.elapsed().as_micros()
                );
            }
        }
    }

    #[test]
    #[ignore = "explicit avatar raster CPU benchmark"]
    fn avatar_raster_benchmark() {
        use std::{hint::black_box, time::Instant};
        for side in [32, 48, 128, 256] {
            for _ in 0..3 {
                let start = Instant::now();
                for i in 0..60 {
                    let anim = BotAnimState {
                        yaw: (i as f32 * 0.37).sin() * 0.5,
                        pitch: (i as f32 * 0.21).cos() * 0.4,
                        ..Default::default()
                    };
                    black_box(render_badge_or_bot(
                        true, "squircle", "dots", false, true, 7, 4, 0x223344, 0xddeeff, side, anim,
                    ));
                }
                eprintln!(
                    "avatar side={side} samples=60 per_frame_us={}",
                    start.elapsed().as_micros() / 60
                );
            }
        }
    }

    #[test]
    fn delete_hover_renders_crossed_eyes_without_changing_the_badge() {
        let render = |crossed_eyes| {
            render_badge_or_bot(
                true,
                "squircle",
                "dots",
                false,
                false,
                12345,
                4,
                0x222222,
                0xffffff,
                200,
                BotAnimState {
                    crossed_eyes,
                    ..Default::default()
                },
            )
        };
        let normal = render(false);
        let crossed = render(true);
        // Two diagonal strokes meet at the left eye; the side of the former
        // round eye is now clear. These are artwork coordinates at 200px.
        for (x, y) in [(77, 99), (87, 109), (87, 89)] {
            assert!(crossed.get_pixel(x, y).0[0] > 200);
        }
        assert!(crossed.get_pixel(87, 99).0[0] < 100);
        assert!(normal.get_pixel(87, 99).0[0] > 200);
        assert_eq!(normal.get_pixel(40, 40), crossed.get_pixel(40, 40));
        for (normal, crossed) in normal.pixels().zip(crossed.pixels()) {
            assert_eq!(normal.0[3], crossed.0[3]);
        }
    }

    #[test]
    fn dither_matches_the_react_workers_palette_and_two_pixel_bayer_cells() {
        let source = RgbaImage::from_pixel(9, 9, image::Rgba([130, 180, 210, 127]));
        let result = background_texture(&source, Effect::Dither);
        let bayer = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
        for (x, y, pixel) in result.enumerate_pixels() {
            let expected = if bayer[(y / 2 % 4) as usize][(x / 2 % 4) as usize] <= 12 {
                [158, 219, 255, 127]
            } else {
                [10, 14, 17, 127]
            };
            assert_eq!(pixel.0, expected, "React worker sample at {x},{y}");
        }
        assert_eq!(background_texture(&source, Effect::None), source);
        assert_eq!(background_texture(&source, Effect::GradientBlur), source);
    }
    #[test]
    fn dither_texture_scales_with_the_artwork_in_the_preview() {
        let source = RgbaImage::from_pixel(128, 64, image::Rgba([130, 180, 210, 255]));
        let full = render_background(&source, Effect::Dither, 128, 64, true);
        let preview = render_background(&source, Effect::Dither, 64, 32, true);
        let reduced = imageops::resize(&full, 64, 32, imageops::FilterType::Triangle);
        for x in 0..64 {
            assert_eq!(
                preview.get_pixel(x, 0).0[..3],
                reduced.get_pixel(x, 0).0[..3]
            );
        }
    }
    #[test]
    fn effects_keep_cover_dimensions_fade_and_change_pixels() {
        let source = RgbaImage::from_fn(64, 48, |x, y| {
            image::Rgba([(x * 4) as u8, (y * 5) as u8, 90, 255])
        });
        let plain = render_background(&source, Effect::None, 41, 33, true);
        for effect in Effect::ALL {
            let result = render_background(&source, effect, 41, 33, true);
            assert_eq!(result.dimensions(), (41, 33));
            assert_eq!(result.get_pixel(0, 0)[3], 255);
            assert_eq!(result.get_pixel(20, 32)[3], 0);
            if effect != Effect::None {
                assert_ne!(result, plain, "{effect:?}");
            }
        }
        let dark = RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 100]));
        assert_eq!(
            render_background(&dark, Effect::Dither, 4, 4, true)
                .get_pixel(0, 0)
                .0,
            [0, 0, 0, 100]
        );
    }
    #[test]
    fn project_settings_normalize_and_round_trip_without_crossing_projects() {
        let mut appearance = ProjectAppearance {
            name: "  My project  ".into(),
            color: Some(0xff000000),
            mascot: Some("missing".into()),
            background: Some(Background {
                path: "a.png".into(),
                empty_opacity: f32::NAN,
                session_opacity: 4.,
                scope: Scope::Empty,
                ..Default::default()
            }),
            ..Default::default()
        };
        appearance.normalize();
        assert_eq!(appearance.label("/tmp/project"), "My project");
        assert_eq!(appearance.color, None);
        assert_eq!(appearance.mascot, None);
        let background = appearance.background.as_ref().unwrap();
        assert_eq!(background.opacity(true), 0.24);
        assert_eq!(background.opacity(false), 0.);
        assert_eq!(background.session_opacity, 0.65);
        assert_eq!(
            serde_json::from_str::<ProjectAppearance>(&serde_json::to_string(&appearance).unwrap())
                .unwrap(),
            appearance
        );
    }
    #[test]
    fn importing_owns_the_image_and_rejects_invalid_files() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("original.png");
        RgbaImage::from_pixel(12, 8, image::Rgba([100, 180, 90, 255]))
            .save(&source)
            .unwrap();
        let imported =
            import_image(&source, &directory.path().join("assets"), "/project", false).unwrap();
        std::fs::remove_file(source).unwrap();
        assert_eq!(
            read_image(Path::new(&imported)).unwrap().dimensions(),
            (12, 8)
        );
        let rotated = directory.path().join("rotated.png");
        RgbaImage::from_pixel(8, 12, image::Rgba([100, 180, 90, 255]))
            .save(&rotated)
            .unwrap();
        let other = import_image(
            &rotated,
            &directory.path().join("assets"),
            "/project",
            false,
        )
        .unwrap();
        assert_ne!(
            imported, other,
            "equal pixel bytes with a different shape need separate assets"
        );
        assert_eq!(read_image(Path::new(&other)).unwrap().dimensions(), (8, 12));
        let bad = directory.path().join("bad.png");
        std::fs::write(&bad, b"not an image").unwrap();
        assert!(import_image(&bad, &directory.path().join("assets"), "/project", false).is_err());
    }
}
