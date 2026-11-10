//! Boundary between decoded straight-alpha pixels and GPUI's BGRA sprites.
use gpui::RenderImage;
use image::{Frame, RgbaImage};
use std::sync::Arc;

pub(crate) fn swap_red_blue(pixels: &mut RgbaImage) {
    for pixel in pixels.as_mut().as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
}

pub(crate) fn to_render_image(mut pixels: RgbaImage) -> Arc<RenderImage> {
    swap_red_blue(&mut pixels);
    Arc::new(RenderImage::new(vec![Frame::new(pixels)]))
}
