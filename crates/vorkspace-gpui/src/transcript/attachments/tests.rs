use super::*;
use gpui::{TestAppContext, WindowOptions};
use image::{ImageBuffer, ImageFormat, Rgba};
use std::{cell::Cell, rc::Rc};

#[gpui::test]
fn pending_thumbnail_reservations_bound_out_of_order_decode_publication(cx: &mut TestAppContext) {
    let view = cx.new(|_| TranscriptView::new());
    let pending = view.update(cx, |view, cx| {
        (0..32)
            .map(|ix| {
                let photo = cx.new(|_| Photo {
                    data: Arc::from(format!("file:{ix}")),
                    mime: "image/gif".into(),
                    loading: true,
                    image: None,
                });
                let weak = photo.downgrade();
                view.cache_photo(ix.to_string(), photo, cx);
                assert!(
                    view.attachment_images
                        .values()
                        .map(|p| p.read(cx).retained_bytes())
                        .sum::<usize>()
                        <= THUMBNAIL_CACHE_BYTES
                );
                weak
            })
            .collect::<Vec<_>>()
    });
    // Complete in reverse order, after every insertion has happened. Each
    // decoder publishes its largest permitted animation through the real owner.
    let mut completed = 0;
    for weak in pending.into_iter().rev() {
        if let Some(photo) = weak.upgrade() {
            let frames: Vec<_> = (0..MAX_GIF_THUMBNAIL_FRAMES)
                .map(|_| {
                    Frame::new(ImageBuffer::from_pixel(
                        THUMBNAIL_MAX_SIDE,
                        THUMBNAIL_MAX_SIDE,
                        Rgba([1, 2, 3, 255]),
                    ))
                })
                .collect();
            photo.update(cx, |photo, cx| {
                photo.decoded(Some(Arc::new(RenderImage::new(frames))), cx)
            });
            view.update(cx, |view, cx| {
                assert!(
                    view.attachment_images
                        .values()
                        .map(|p| p.read(cx).retained_bytes())
                        .sum::<usize>()
                        <= THUMBNAIL_CACHE_BYTES
                );
            });
            completed += 1;
        }
    }
    assert_eq!(
        completed,
        THUMBNAIL_CACHE_BYTES / (MAX_THUMBNAIL_BYTES + "file:31".len())
    );
    let failed = cx.new(|_| Photo {
        data: Arc::from("file:failed"),
        mime: "image/gif".into(),
        loading: true,
        image: None,
    });
    failed.update(cx, |photo, cx| {
        assert_eq!(
            photo.retained_bytes(),
            MAX_THUMBNAIL_BYTES + photo.data.len()
        );
        photo.decoded(None, cx);
        assert_eq!(photo.retained_bytes(), photo.data.len());
    });
}

#[test]
fn decoded_attachment_thumbnail_bounds_both_retained_dimensions() {
    // This panorama's source RGBA pixels occupy 16 MiB. The retained
    // thumbnail must stay at or below 108x108 even for extreme ratios.
    let source = ImageBuffer::from_pixel(4096, 1024, Rgba([10, 20, 30, 255]));
    let mut encoded = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(source)
        .write_to(&mut encoded, ImageFormat::Png)
        .unwrap();

    let thumbnail = decode_attachment_thumbnail(encoded.get_ref(), "image/png").unwrap();
    let size = thumbnail.size(0);
    assert!(size.width.0 <= THUMBNAIL_MAX_SIDE as i32);
    assert!(size.height.0 <= THUMBNAIL_MAX_SIDE as i32);
    assert!(thumbnail.as_bytes(0).unwrap().len() <= (108 * 108 * 4) as usize);
    assert!(thumbnail.as_bytes(0).unwrap().len() < 4096 * 1024 * 4);
    let duplicate = decode_attachment_thumbnail(encoded.get_ref(), "image/png").unwrap();
    assert_ne!(thumbnail.id, duplicate.id, "each Photo owns its own sprite");
}

#[test]
fn decoder_rejects_sources_over_the_dimension_limit() {
    let source = ImageBuffer::from_pixel(MAX_DECODED_DIMENSION + 1, 1, Rgba([10, 20, 30, 255]));
    let mut encoded = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(source)
        .write_to(&mut encoded, ImageFormat::Png)
        .unwrap();

    assert!(decode_attachment_thumbnail(encoded.get_ref(), "image/png").is_err());
}

#[test]
fn decoded_source_pixel_budget_is_checked_before_allocation() {
    assert!(check_decode_dimensions(4096, 4096).is_ok());
    assert!(check_decode_dimensions(8193, 4096).is_err());
    assert!(check_decode_dimensions(MAX_DECODED_DIMENSION + 1, 1).is_err());
}

#[test]
fn animated_thumbnail_keeps_frame_delays_and_bgra_pixels() {
    let frame =
        |color, delay| Frame::from_parts(ImageBuffer::from_pixel(16, 8, Rgba(color)), 0, 0, delay);
    let first_delay = image::Delay::from_numer_denom_ms(40, 1);
    let second_delay = image::Delay::from_numer_denom_ms(80, 1);
    let image = bounded_animation(
        vec![
            Ok(frame([10, 20, 30, 255], first_delay)),
            Ok(frame([40, 50, 60, 255], second_delay)),
        ]
        .into_iter(),
    )
    .unwrap();

    assert_eq!(image.frame_count(), 2);
    assert_eq!(image.size(0).width.0, 16);
    assert_eq!(image.delay(0), first_delay);
    assert_eq!(image.delay(1), second_delay);
    assert_eq!(&image.as_bytes(0).unwrap()[..4], &[30, 20, 10, 255]);
}

#[test]
fn path_photos_match_by_path_even_when_the_arc_is_recreated() {
    let photo = Photo {
        data: Arc::from("/tmp/photo.png"),
        loading: false,
        mime: "image/png".into(),
        image: None,
    };
    assert!(photo.matches_source(&Arc::from("/tmp/photo.png"), &"image/png".into()));
    assert!(!photo.matches_source(&Arc::from("/tmp/other.png"), &"image/png".into()));
}

#[gpui::test]
fn releasing_a_photo_drops_its_unique_render_sprite(cx: &mut TestAppContext) {
    struct Host(Option<gpui::Entity<Photo>>);
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().children(self.0.iter().cloned())
        }
    }

    let image = Arc::new(RenderImage::new(vec![Frame::new(ImageBuffer::from_pixel(
        16,
        16,
        Rgba([10, 20, 30, 255]),
    ))]));
    let released = Rc::new(Cell::new(false));
    let photo = cx.new(|cx| {
        let released = released.clone();
        cx.on_release(move |photo: &mut Photo, cx| {
            released.set(true);
            if let Some(image) = photo.image.take() {
                cx.drop_image(image, None);
            }
        })
        .detach();
        Photo {
            data: Arc::from("owned-test-image"),
            loading: false,
            mime: "image/png".into(),
            image: Some(image.clone()),
        }
    });
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| Host(Some(photo.clone())))
        })
        .unwrap()
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    let rendered = window
        .update(cx, |_, window, _| window.has_image_atlas_entry(&image))
        .unwrap();
    assert!(
        rendered,
        "the photo should own a live atlas entry after drawing"
    );

    window
        .update(cx, |host, _, cx| {
            host.0 = None;
            cx.notify();
        })
        .unwrap();
    drop(photo);
    cx.update_window(window.into(), |_, window, cx| {
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    assert!(
        released.get(),
        "dropping the last Photo handle runs cleanup"
    );
    let retained = window
        .update(cx, |_, window, _| window.has_image_atlas_entry(&image))
        .unwrap();
    assert!(!retained, "releasing the owner removes its atlas entry");
}
