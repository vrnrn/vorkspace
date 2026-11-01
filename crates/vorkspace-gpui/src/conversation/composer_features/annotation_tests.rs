use super::*;
use gpui::TestAppContext;
use vorkspace_core::store::Store;

#[gpui::test]
fn failed_draft_commit_restores_attachment_and_retains_recovery(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("vorkspace.db");
    let source = dir.path().join("source.png");
    std::fs::write(&source, b"original").unwrap();
    let store = Store::open_vorkspace(&db_path).unwrap();
    let session = store.create("codex", "/missing/annotation-test").unwrap();
    let id = session.id;
    let db = Arc::new(std::sync::Mutex::new(store));
    let chat = cx.new(|cx| Conversation::new(session, db.clone(), cx));
    cx.run_until_parked();
    let generation = chat.update(cx, |chat, cx| {
        chat.attachments.push(source.clone());
        chat.capture_composer(cx);
        chat.composer_draft.flush().unwrap();
        chat.attachment_draft_generation()
    });
    let external = Store::open_vorkspace(&db_path).unwrap();
    let draft = external.composer_draft(id).unwrap().unwrap();
    external
        .save_composer_draft(id, draft.revision, &draft.payload)
        .unwrap();
    chat.update(cx, |chat, cx| {
        assert!(
            chat.apply_annotated_attachment(&source, generation, b"new image", cx)
                .is_err()
        );
        assert_eq!(chat.attachments, vec![source.clone()]);
        assert!(chat.composer_draft.error.is_some());
    });
    let draft = external.composer_draft(id).unwrap().unwrap();
    assert_eq!(
        draft.payload["attachments"][0]["path"],
        source.to_str().unwrap()
    );
    assert_eq!(std::fs::read(&source).unwrap(), b"original");
    assert!(!external.composer_recoveries(id).unwrap().is_empty());
}

#[gpui::test]
fn drawing_save_control_publishes_pixels_to_the_draft(cx: &mut TestAppContext) {
    use gpui::{MouseButton, VisualTestContext, point, px};
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.png");
    image::RgbaImage::from_pixel(100, 80, image::Rgba([255, 255, 255, 255]))
        .save(&source)
        .unwrap();
    let original = std::fs::read(&source).unwrap();
    let store = Store::open_vorkspace(&dir.path().join("vorkspace.db")).unwrap();
    let session = store.create("codex", "/missing/annotation-test").unwrap();
    let id = session.id;
    let db = Arc::new(std::sync::Mutex::new(store));
    let owner = cx.new(|cx| Conversation::new(session, db.clone(), cx));
    cx.run_until_parked();
    owner.update(cx, |view, cx| {
        view.attachments.push(source.clone());
        view.capture_composer(cx);
        view.composer_draft.flush().unwrap();
    });
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |window, cx| {
            cx.new(|cx| {
                crate::image_lightbox::ImageLightbox::new(
                    crate::image_lightbox::Open {
                        source: crate::image_lightbox::Source::Path(source.clone()),
                        alt: "Source".into(),
                        draft_session: Some(id),
                    },
                    (Some(owner.clone()))
                        .as_ref()
                        .map(crate::conversation::composer_features::annotation_owner),
                    window,
                    cx,
                )
            })
        })
        .unwrap()
    });
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.refresh().unwrap();
    let brush = visual.debug_bounds("annotation-brush").unwrap().center();
    visual.simulate_click(brush, Default::default());
    let bounds = visual.debug_bounds("lightbox-image").unwrap();
    let start = point(bounds.left() + px(25.), bounds.top() + px(40.));
    let end = point(bounds.left() + px(75.), bounds.top() + px(40.));
    visual.simulate_mouse_down(start, MouseButton::Left, Default::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Default::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Default::default());
    let save = visual.debug_bounds("annotation-save").unwrap().center();
    visual.simulate_click(save, Default::default());
    visual.run_until_parked();
    let path = owner.read_with(visual, |view, _| view.attachments[0].clone());
    assert_ne!(path, source);
    assert_eq!(std::fs::read(&source).unwrap(), original);
    let image = image::open(&path).unwrap().into_rgba8();
    assert_eq!(image.get_pixel(50, 40).0, [255, 59, 48, 255]);
    let draft = locked(&db).unwrap().composer_draft(id).unwrap().unwrap();
    assert_eq!(
        draft.payload["attachments"][0]["path"],
        path.to_str().unwrap()
    );
}

#[gpui::test]
fn close_controls_save_to_original_draft_and_stale_draft_keeps_preview(cx: &mut TestAppContext) {
    use gpui::{VisualTestContext, point, px};
    cx.update(crate::input::bind);
    for (outside, stale) in [(false, false), (true, false), (false, true)] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.png");
        image::RgbaImage::from_pixel(120, 80, image::Rgba([255, 255, 255, 255]))
            .save(&source)
            .unwrap();
        let original = std::fs::read(&source).unwrap();
        let store = Store::open_vorkspace(&dir.path().join("draft.db")).unwrap();
        let session = store.create("codex", "/missing/close-test").unwrap();
        let id = session.id;
        let db = Arc::new(std::sync::Mutex::new(store));
        let owner = cx.new(|cx| Conversation::new(session, db.clone(), cx));
        cx.run_until_parked();
        owner.update(cx, |view, cx| {
            view.attachments.push(source.clone());
            view.capture_composer(cx);
            view.composer_draft.flush().unwrap();
        });
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |window, cx| {
                cx.new(|cx| {
                    crate::image_lightbox::ImageLightbox::new(
                        crate::image_lightbox::Open {
                            source: crate::image_lightbox::Source::Path(source.clone()),
                            alt: "Source".into(),
                            draft_session: Some(id),
                        },
                        (Some(owner.clone()))
                            .as_ref()
                            .map(crate::conversation::composer_features::annotation_owner),
                        window,
                        cx,
                    )
                })
            })
            .unwrap()
        });
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.refresh().unwrap();
        let brush = visual.debug_bounds("annotation-brush").unwrap().center();
        visual.simulate_click(brush, Default::default());
        let bounds = visual.debug_bounds("lightbox-image").unwrap();
        visual.simulate_click(bounds.center(), Default::default());
        let text_tool = visual.debug_bounds("annotation-text").unwrap().center();
        visual.simulate_click(text_tool, Default::default());
        visual.simulate_click(
            point(bounds.left() + px(10.), bounds.top() + px(10.)),
            Default::default(),
        );
        visual.simulate_input("OK");
        if stale {
            owner.update(visual, |view, cx| {
                view.input
                    .update(cx, |input, cx| input.set_text("Changed draft".into(), cx));
            });
        }
        let target = if outside {
            point(px(4.), px(4.))
        } else {
            visual.debug_bounds("close-image-preview").unwrap().center()
        };
        visual.simulate_click(target, Default::default());
        // Repeated close requests cannot launch another publication.
        visual.simulate_click(target, Default::default());
        visual.run_until_parked();
        visual.refresh().unwrap();
        let attachment = owner.read_with(visual, |view, _| view.attachments[0].clone());
        assert_eq!(std::fs::read(&source).unwrap(), original);
        if stale {
            assert_eq!(attachment, source);
            assert!(visual.debug_bounds("annotation-error").is_some());
            assert!(visual.debug_bounds("lightbox-image").is_some());
            assert!(visual.debug_bounds("annotation-text-box-0").is_some());
        } else {
            assert_ne!(attachment, source);
            assert_eq!(
                image::open(&attachment)
                    .unwrap()
                    .into_rgba8()
                    .get_pixel(60, 40)
                    .0,
                [255, 59, 48, 255]
            );
            let flattened = image::open(&attachment).unwrap().into_rgba8();
            assert!((10..35).any(|y| {
                (10..70).any(|x| flattened.get_pixel(x, y).0 != [255, 255, 255, 255])
            }));
            let draft = locked(&db).unwrap().composer_draft(id).unwrap().unwrap();
            assert_eq!(
                draft.payload["attachments"][0]["path"],
                attachment.to_str().unwrap()
            );
        }
        window
            .update(visual, |_, window, _| window.remove_window())
            .unwrap();
    }
}

#[gpui::test]
fn annotated_copy_is_persisted_and_sent_and_stale_owner_is_rejected(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("vorkspace.db");
    let source = dir.path().join("source.png");
    image::RgbaImage::from_pixel(8, 8, image::Rgba([255, 255, 255, 255]))
        .save(&source)
        .unwrap();
    let original = std::fs::read(&source).unwrap();
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        8,
        8,
        image::Rgba([255, 59, 48, 255]),
    ))
    .write_to(&mut encoded, image::ImageFormat::Png)
    .unwrap();
    let bytes = encoded.into_inner();
    let store = Store::open_vorkspace(&db_path).unwrap();
    let session = store.create("codex", "/missing/annotation-test").unwrap();
    let id = session.id;
    let db = Arc::new(std::sync::Mutex::new(store));
    let chat = cx.new(|cx| Conversation::new(session.clone(), db.clone(), cx));
    cx.run_until_parked();
    let annotated = chat.update(cx, |chat, cx| {
        chat.attachments.push(source.clone());
        chat.input
            .update(cx, |input, cx| input.set_text("Keep this draft".into(), cx));
        chat.capture_composer(cx);
        chat.composer_draft.flush().unwrap();
        let generation = chat.attachment_draft_generation();
        let too_large = vec![0; vorkspace_core::composer::MAX_EMBED_BYTES as usize + 1];
        assert!(
            chat.apply_annotated_attachment(&source, generation, &too_large, cx)
                .is_err()
        );
        assert_eq!(chat.attachments, vec![source.clone()]);
        let path = chat
            .apply_annotated_attachment(&source, generation, &bytes, cx)
            .unwrap();
        assert!(
            chat.apply_annotated_attachment(&source, generation, b"stale", cx)
                .is_err()
        );
        path
    });
    assert_eq!(std::fs::read(&source).unwrap(), original);
    assert_eq!(std::fs::read(&annotated).unwrap(), bytes);
    let wire_image = vorkspace_core::composer::image_data(&annotated).unwrap();
    assert_eq!(
        vorkspace_core::composer::decode_image_data(&wire_image).unwrap(),
        bytes
    );
    let draft = locked(&db).unwrap().composer_draft(id).unwrap().unwrap();
    assert_eq!(
        draft.payload["attachments"][0]["path"],
        annotated.to_str().unwrap()
    );
    let reopened = cx.new(|cx| Conversation::new(session, db.clone(), cx));
    cx.run_until_parked();
    reopened.update(cx, |chat, cx| {
        assert_eq!(chat.attachments, vec![annotated.clone()]);
        assert_eq!(chat.input.read(cx).content, "Keep this draft");
    });
    drop(reopened);
    chat.update(cx, |chat, cx| {
        chat.loaded = true;
        chat.send(cx);
        let rows = chat.transcript.read(cx).source_rows();
        let manifest = rows
            .iter()
            .find(|row| row.role == vorkspace_core::transcript::attachments::ROLE)
            .unwrap();
        assert_eq!(
            vorkspace_core::transcript::attachments::paths(manifest).unwrap(),
            &[annotated]
        );
    });
}
