//! Rasterize the static body shared by every pose of a Bot.
use super::{normalize_badge_shape, normalize_bot_pattern, normalize_bot_print};
use image::RgbaImage;

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

#[inline]
fn sd_rounded_box(px: f32, py: f32, bx: f32, by: f32, r: f32) -> f32 {
    let qx = px.abs() - bx + r;
    let qy = py.abs() - by + r;
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// Signed distance to a badge shape centered at `(s/2, s/2)`.
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
            // Rotate by 30 degrees to fold the pointy-top hexagon.
            let mut qx = (cx * 0.866_025_4 - cy * 0.5).abs();
            let mut qy = (cx * 0.5 + cy * 0.866_025_4).abs();
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
        "octagon" => {
            // Regular octagon with softened corners.
            let cr = s * 0.06;
            let r = s * 0.46 - cr;
            let (kx, ky, kz) = (-0.923_879_5_f32, 0.382_683_43_f32, 0.414_213_57_f32);
            let (mut qx, mut qy) = (cx.abs(), cy.abs());
            let m = 2.0 * (kx * qx + ky * qy).min(0.0);
            qx -= m * kx;
            qy -= m * ky;
            let m = 2.0 * (-kx * qx + ky * qy).min(0.0);
            qx -= m * -kx;
            qy -= m * ky;
            qx -= qx.clamp(-kz * r, kz * r);
            qy -= r;
            qx.hypot(qy) * qy.signum() - cr
        }
        "ghost" => {
            // A dome over a straight-sided body with soft feet.
            let dome = cx.hypot(cy + s * 0.04) - s * 0.42;
            let body = sd_rounded_box(cx, cy - s * 0.2, s * 0.42, s * 0.24, s * 0.08);
            dome.min(body)
        }
        "tv" => sd_rounded_box(cx, cy, s * 0.47, s * 0.37, s * 0.13),
        _ => {
            // "squircle"
            let half = s * 0.46;
            sd_rounded_box(cx, cy, half, half, s * 0.30)
        }
    }
}

/// A Bot's body without its face: the shape in its color, the pattern
/// printed as a ground that rises from the bottom edge, and a quiet rim. It
/// depends only on the look and the pixel size, never on pose, so it is
/// rasterized once per look and reused for every frame; the face, gaze,
/// blink and motion are drawn over it as vector shapes.
#[allow(clippy::too_many_arguments)]
pub fn render_bot_body(
    shape: &str,
    pattern: &str,
    print: &str,
    textured: bool,
    seed: u32,
    grid: u8,
    bg_hex: u32,
    fg_hex: u32,
    pixel_side: u32,
) -> RgbaImage {
    let side = pixel_side.max(16);
    let s = side as f32;
    let shape_id = normalize_badge_shape(shape);
    let pattern = if textured {
        normalize_bot_pattern(pattern)
    } else {
        "solid"
    };
    let print = normalize_bot_print(print);
    let bg = unpack_rgb(bg_hex);
    let fg = unpack_rgb(fg_hex);
    const BAYER: [f32; 16] = [
        0., 8., 2., 10., 12., 4., 14., 6., 3., 11., 1., 9., 15., 7., 13., 5.,
    ];
    // Print cells in device pixels: grid 2 is bold, grid 8 is fine.
    let cell = (s / (grid.clamp(2, 8) as f32 * 7.)).max(1.);
    let hash = |x: u32, y: u32| -> f32 {
        let mut h = x.wrapping_mul(0x9e37_79b1) ^ y.wrapping_mul(0x85eb_ca77) ^ seed;
        h ^= h >> 15;
        h = h.wrapping_mul(0x2c1b_3c6d);
        h ^= h >> 12;
        (h & 0xffff) as f32 / 65535.0
    };
    let border_w = (s * 0.025).max(1.0);
    let mut out = RgbaImage::new(side, side);
    for py in 0..side {
        let fy = py as f32 + 0.5;
        // Ground: rising from nothing at 40% to full at the base. Cap: the
        // same from the top. Full: an even print across the body.
        let rise = match print {
            "cap" => ((0.6 - fy / s) / 0.6).clamp(0., 1.),
            "full" => 0.7,
            _ => ((fy / s - 0.4) / 0.6).clamp(0., 1.),
        };
        for px in 0..side {
            let fx = px as f32 + 0.5;
            let d = sd_badge_shape(shape_id, fx, fy, s);
            let alpha = (0.5 - d).clamp(0.0, 1.0);
            if alpha <= 0.0 {
                continue;
            }
            let (cx, cy) = ((fx / cell).floor(), (fy / cell).floor());
            let ink = match pattern {
                // Quarter tone at most: Bayer ranks below 4 are an even dot
                // grid, never the half-tone checkerboard.
                "dither" => {
                    let rank = BAYER[(cy as usize % 4) * 4 + cx as usize % 4];
                    if rank < 4. * rise {
                        1.
                    } else {
                        0.
                    }
                }
                // Round dots on a 45 degree screen, growing toward the bottom.
                "halftone" => {
                    let sp = cell * 2.2 * std::f32::consts::SQRT_2;
                    let (r, t) = ((fx + fy) / sp, (fx - fy) / sp);
                    let (fr, ft) = (r.fract() - 0.5, t.fract() - 0.5);
                    let dist = (fr * fr + ft * ft).sqrt();
                    let radius = 0.4 * rise.sqrt();
                    if rise > 0. {
                        ((radius - dist) * sp + 0.5).clamp(0., 1.)
                    } else {
                        0.
                    }
                }
                "grain" => {
                    if hash(cx as u32, cy as u32) < 0.32 * rise {
                        1.
                    } else {
                        0.
                    }
                }
                // Horizontal rules that thicken toward the bottom.
                "stripes" => {
                    let period = cell * 2.6;
                    let t = (fy / period).fract();
                    if t < 0.5 * rise {
                        1.
                    } else {
                        0.
                    }
                }
                _ => 0.,
            };
            let mut col = lerp_rgb(bg, fg, ink * 0.55);
            let d_border = (d + border_w * 0.5).abs() - border_w * 0.5;
            let rim = (0.5 - d_border).clamp(0.0, 1.0) * 0.14;
            if rim > 0.0 {
                col = lerp_rgb(col, [255.0, 255.0, 255.0], rim);
            }
            out.put_pixel(
                px,
                py,
                image::Rgba([
                    col[0].round() as u8,
                    col[1].round() as u8,
                    col[2].round() as u8,
                    (alpha * 255.).round() as u8,
                ]),
            );
        }
    }
    out
}
