use super::*;
#[gpui::test]
fn review_overview_has_no_change_markers_and_supports_scrolling_and_dragging(
    cx: &mut gpui::TestAppContext,
) {
    let original: String = (0..200).map(|n| format!("line {n}\n")).collect();
    let current = original
        .replace("line 80\n", "changed 80\n")
        .replace("line 150\n", "changed 150\n");
    let unified = diff::build_unified_file(&original, &current, usize::MAX);
    let mut model = file("overview", &original, &current);
    model.blocks = Arc::new(unified.blocks);
    let window = cx.update(|cx| {
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(gpui::WindowBounds::Windowed(Bounds::new(
                    gpui::point(px(0.), px(0.)),
                    gpui::size(px(500.), px(300.)),
                ))),
                ..Default::default()
            },
            |_, cx| cx.new(DiffView::new),
        )
        .unwrap()
    });
    window
        .update(cx, |view, _, cx| view.set_files(vec![model], cx))
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let rail = visual.debug_bounds("diff-overview").unwrap();
    assert_eq!(rail.size.width, px(18.));
    window
        .update(visual, |_, window, cx| {
            let (add, del) = crate::file_tools::editor_scrollbar::diff_colors(cx);
            let scale = window.scale_factor();
            let quads = window.painted_quads();
            let rail_quads: Vec<_> = quads
                .iter()
                .filter(|quad| {
                    quad.bounds.left().0 >= rail.left().as_f32() * scale
                        && quad.bounds.right().0 <= rail.right().as_f32() * scale
                })
                .collect();
            assert!(!rail_quads.is_empty());
            assert!(rail_quads.iter().all(|quad| {
                quad.background != gpui::Hsla::from(rgb(add)).into()
                    && quad.background != gpui::Hsla::from(rgb(del)).into()
            }));
        })
        .unwrap();
    let track = rail.origin + gpui::point(px(8.), rail.size.height * 0.75);
    visual.simulate_mouse_down(track, MouseButton::Left, Default::default());
    window
        .update(visual, |view, _, _| {
            assert!(view.overview_drag.is_some());
        })
        .unwrap();
    visual.simulate_mouse_up(track, MouseButton::Left, Default::default());
    visual.run_until_parked();
    let thumb = window
        .update(visual, |view, _, _| {
            assert!(view.top_px() > 2500.);
            let m = view.overview_metrics(rail.size.height.as_f32());
            rail.origin + gpui::point(px(8.), px(m.thumb_top + 4.))
        })
        .unwrap();
    visual.simulate_mouse_down(thumb, MouseButton::Left, Default::default());
    let bottom = gpui::point(thumb.x, rail.bottom() - px(1.));
    visual.simulate_mouse_move(bottom, MouseButton::Left, Default::default());
    visual.simulate_mouse_up(bottom, MouseButton::Left, Default::default());
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            assert!(view.top_px() > 3500.);
            assert!(view.overview_drag.is_none());
        })
        .unwrap();
}
fn file(id: &str, original: &str, current: &str) -> DiffFile {
    let unified = diff::build_unified_file(original, current, 3);
    DiffFile {
        id: id.to_owned().into(),
        path: format!("/r/{id}"),
        label: id.into(),
        binary: false,
        too_large: false,
        empty_message: None,
        additions: unified.additions,
        deletions: unified.deletions,
        blocks: Arc::new(unified.blocks),
        can_stage: true,
        can_discard: true,
        can_stage_hunk: true,
    }
}
#[gpui::test]
fn sideways_swipes_end_jumps_and_never_scroll_the_list(cx: &mut gpui::TestAppContext) {
    use gpui::{ScrollDelta, ScrollWheelEvent, TouchPhase, point};
    let wide: String = (0..80)
        .map(|i| format!("{i} {}\n", "x".repeat(300)))
        .collect();
    let edited = wide.replacen("1 x", "1 y", 1);
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| cx.new(DiffView::new))
            .unwrap()
    });
    let cx = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(cx, |view, _, cx| {
            let mut loading = file("b.rs", "", "");
            loading.empty_message = Some("Loading…".into());
            view.set_files(vec![file("a.rs", &wide, &edited), loading], cx);
            view.focus_file(Some("b.rs".into()), cx);
            assert!(view.pending.is_some());
        })
        .unwrap();
    cx.run_until_parked();
    let top = window.update(cx, |view, _, _| view.top_px()).unwrap();
    for (phase, dx, dy) in [
        (TouchPhase::Started, -40., -3.),
        (TouchPhase::Moved, -40., 6.),
        (TouchPhase::Moved, -40., 8.),
    ] {
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(300.), px(200.)),
            delta: ScrollDelta::Pixels(point(px(dx), px(dy))),
            modifiers: Default::default(),
            touch_phase: phase,
        });
    }
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| {
            assert!(view.pending.is_none());
            assert_eq!(view.scroll_x, 120.);
            assert_eq!(view.top_px(), top);
        })
        .unwrap();
}
#[gpui::test]
fn sticky_header_retains_its_translucent_blur_surface(cx: &mut gpui::TestAppContext) {
    let long: String = (0..60).map(|i| format!("line {i}\n")).collect();
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| cx.new(DiffView::new))
            .unwrap()
    });
    window
        .update(cx, |view, _, cx| {
            view.set_files(
                vec![file("a.rs", &long, &long.replace("line 30", "LINE 30"))],
                cx,
            );
            view.list.scroll_to(ListOffset {
                item_ix: 3,
                offset_in_item: px(0.),
            });
        })
        .unwrap();
    cx.refresh().unwrap();
    window
        .update(cx, |_, window, _| {
            let marker =
                gpui_apple::metal_renderer::backdrop_blur_marker(24. * window.scale_factor());
            let quads = window.painted_quads();
            let glass = quads
                .iter()
                .find(|q| q.border_color == marker)
                .expect("the sticky header's blur must survive GPUI's transparent-quad culling");
            assert_eq!(glass.background, content(0.02).into());
            assert_eq!(
                glass.bounds.size.height.0,
                FILE_HEADER * window.scale_factor()
            );
        })
        .unwrap();
}
#[gpui::test]
fn pinned_header_follows_open_files_only(cx: &mut gpui::TestAppContext) {
    let long: String = (0..60).map(|i| format!("line {i}\n")).collect();
    let edited = long.replace("line 30", "LINE 30");
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| cx.new(DiffView::new))
            .unwrap()
    });
    window
        .update(cx, |view, _, cx| {
            view.set_files(
                vec![file("a.rs", &long, &edited), file("b.rs", "x\n", "y\n")],
                cx,
            );
            // Inside a.rs's body, below its header.
            view.list.scroll_to(ListOffset {
                item_ix: 3,
                offset_in_item: px(0.),
            });
            assert!(view.sticky_header(cx).is_some());
            view.toggle(0, cx);
            view.list.scroll_to(ListOffset {
                item_ix: 0,
                offset_in_item: px(10.),
            });
            assert!(view.sticky_header(cx).is_none());
        })
        .unwrap();
}
#[test]
fn bodies_number_hunks_and_mark_stageable_lines() {
    let f = file("a.rs", "one\ntwo\nthree\n", "one\nTWO\nthree\n");
    let body = build_body(&f, f.message(), true, Layout::Unified);
    assert!(
        matches!(&body.rows[0].kind, RowKind::Hunk(text) if text.as_ref() == "@@ -1,3 +1,3 @@")
    );
    let stages: Vec<_> = body
        .rows
        .iter()
        .filter_map(|r| match r.kind {
            RowKind::Line { stage, .. } => Some(stage),
            _ => None,
        })
        .collect();
    assert_eq!(stages, [None, Some(0), Some(0), None]);
    let binary = DiffFile {
        binary: true,
        ..f.clone()
    };
    assert!(
        matches!(&build_body(&binary, binary.message(), true, Layout::Unified).rows[0].kind,
            RowKind::Notice(t) if t.as_ref() == "Binary file changed")
    );
    assert_eq!(expand_tabs("\tab\tc"), "    ab  c");
}

#[test]
fn side_by_side_pairs_changes_without_dropping_lines_or_crossing_hunks() {
    for (before, after) in [
        ("", "one\ntwo\n"),
        ("one\ntwo\n", ""),
        ("one\ntwo\nthree\n", "one\nTWO\nextra\nthree\n"),
        ("one\ntwo\nthree\n", "one\nthree\n"),
    ] {
        let file = file("pair.rs", before, after);
        let body = build_body(&file, file.message(), true, Layout::Split);
        let mut left = Vec::new();
        let mut right = Vec::new();
        for row in &body.rows {
            if let RowKind::Split { block, old, new } = row.kind {
                if let Some(line) = old {
                    left.push((block, line));
                }
                if let Some(line) = new {
                    right.push((block, line));
                }
            }
        }
        let visible: Vec<_> = file
            .blocks
            .iter()
            .enumerate()
            .flat_map(|(b, block)| {
                let lines = if let Block::Hunk { lines, .. } = block {
                    lines.as_slice()
                } else {
                    &[]
                };
                lines
                    .iter()
                    .enumerate()
                    .filter(|(_, line)| line.kind != LineKind::Hunk)
                    .map(move |(l, line)| ((b, l), line.kind))
            })
            .collect();
        assert_eq!(
            left,
            visible
                .iter()
                .filter(|(_, kind)| *kind != LineKind::Add)
                .map(|(key, _)| *key)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            right,
            visible
                .iter()
                .filter(|(_, kind)| *kind != LineKind::Del)
                .map(|(key, _)| *key)
                .collect::<Vec<_>>()
        );
    }
}

#[gpui::test]
fn live_layout_switch_retains_hunks_folds_anchor_tokens_and_comment_draft(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| cx.set_global(crate::settings::Preferences::default()));
    let before: String = (0..100).map(|n| format!("line {n}\n")).collect();
    let after = before
        .replace("line 5\n", "changed 5\nextra\n")
        .replace("line 80\n", "changed 80\n");
    let model = file("switch.rs", &before, &after);
    let blocks = model.blocks.clone();
    let id = model.id.clone();
    let window = cx.update(|cx| {
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(gpui::WindowBounds::Windowed(Bounds::new(
                    gpui::point(px(0.), px(0.)),
                    gpui::size(px(900.), px(240.)),
                ))),
                ..Default::default()
            },
            |_, cx| cx.new(DiffView::new),
        )
        .unwrap()
    });
    let (key, draft, hunks) = window.update(cx, |view, w, cx| {
            view.set_files(vec![model, file("closed.rs", "old\n", "new\n")], cx);
            view.toggle(1, cx);
            view.highlights.insert(id.clone(), (blocks.clone(), None));
            let ix = view.rows.iter().position(|row| matches!(row.kind, RowKind::Line { block, line, .. } if view.files[row.file].blocks[block].lines()[line].kind == LineKind::Add)).unwrap();
            let key = view.row_key(ix).unwrap();
            let line = view.files[0].blocks[key.2].lines()[key.3].clone();
            view.open_comment(0, line, AnchorSlot::default(), w, cx);
            let draft = view.comment.as_ref().unwrap().input.clone();
            draft.update(cx, |input, cx| input.set_text("keep this comment".into(), cx));
            view.list.scroll_to(ListOffset { item_ix: ix, offset_in_item: px(3.) });
            (key, draft, view.hunks.len())
        }).unwrap();
    cx.update(|cx| {
        cx.update_global::<crate::settings::Preferences, _>(|p, _| p.diff_view = "split".into())
    });
    window
        .update(cx, |view, _, _| {
            assert_eq!(view.layout, Layout::Split);
            assert_eq!(view.hunks.len(), hunks);
            assert!(!view.open.contains(&SharedString::from("closed.rs")));
            let top = view.list.logical_scroll_top();
            assert!(view.row_matches(top.item_ix, &key));
            assert_eq!(top.offset_in_item, px(3.));
            assert!(Arc::ptr_eq(&view.highlights[&id].0, &blocks));
            assert_eq!(view.comment.as_ref().unwrap().input, draft);
            assert!(view.rows.iter().any(|row| matches!(
                row.kind,
                RowKind::Split {
                    old: Some(_),
                    new: Some(_),
                    ..
                }
            )));
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    let split_ix = window
        .update(cx, |view, _, _| {
            view.rows
                .iter()
                .position(|row| {
                    matches!(
                        row.kind,
                        RowKind::Split {
                            old: Some(_),
                            new: Some(_),
                            ..
                        }
                    )
                })
                .unwrap()
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    // Put the first changed pair on screen before measuring both lanes.
    window
        .update(visual, |view, _, cx| {
            view.list.scroll_to(ListOffset {
                item_ix: split_ix,
                offset_in_item: px(0.),
            });
            cx.notify();
        })
        .unwrap();
    visual.refresh().unwrap();
    visual.run_until_parked();
    let left = visual
        .debug_bounds(Box::leak(format!("diff-old-{split_ix}").into_boxed_str()))
        .unwrap();
    let right = visual
        .debug_bounds(Box::leak(format!("diff-new-{split_ix}").into_boxed_str()))
        .unwrap();
    assert!((left.size.width - right.size.width).abs() <= px(1.));
    assert!((left.right() - right.left()).abs() <= px(1.));
    let viewport = window
        .update(visual, |view, _, _| view.viewport.get().unwrap())
        .unwrap();
    assert!(
        (left.size.width + right.size.width - viewport.size.width).abs()
            <= px(crate::file_tools::editor_scrollbar::WIDTH + 1.)
    );
    window
        .update(visual, |view, _, cx| {
            assert_eq!(draft.read(cx).content.as_ref(), "keep this comment");
            assert!(view.comment.as_ref().unwrap().anchor.get().is_some());
            for row in &view.rows {
                if let RowKind::Split { block, old, new } = row.kind {
                    for line in [old, new].into_iter().flatten() {
                        let l = &view.files[row.file].blocks[block].lines()[line];
                        if matches!(l.kind, LineKind::Add | LineKind::Del) {
                            assert!(view.line_stage(row.file, block, Some(line)).is_some());
                        }
                    }
                }
            }
        })
        .unwrap();
    let hunk_key = window
        .update(visual, |view, _, _| {
            let ix = view
                .rows
                .iter()
                .rposition(|row| matches!(row.kind, RowKind::Hunk(_)))
                .unwrap();
            view.list.scroll_to(ListOffset {
                item_ix: ix,
                offset_in_item: px(2.),
            });
            view.row_key(ix).unwrap()
        })
        .unwrap();
    for mode in ["unified", "split"] {
        visual.update(|_, cx| {
            cx.update_global::<crate::settings::Preferences, _>(|p, _| p.diff_view = mode.into())
        });
        window
            .update(visual, |view, _, cx| {
                let top = view.list.logical_scroll_top();
                assert_eq!(view.row_key(top.item_ix).as_ref(), Some(&hunk_key));
                assert_eq!(top.offset_in_item, px(2.));
                assert_eq!(draft.read(cx).content.as_ref(), "keep this comment");
            })
            .unwrap();
    }
}
