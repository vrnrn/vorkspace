
use super::*;
use gpui::TestAppContext;
#[gpui::test]
fn prepending_history_preserves_the_selected_text(cx: &mut TestAppContext) {
    let view = cx.new(|_| TranscriptView::new());
    view.update(cx, |view, cx| {
        let row = |text: &str| Row {
            role: "assistant".into(),
            text: text.into(),
            first: true,
            sent_at: None,
        };
        view.install_prepared_history(PreparedHistory::new(vec![row("First answer")], false), cx);
        let key = view
            .rows
            .iter()
            .find(|row| matches!(row.body, Body::Markdown(_)))
            .unwrap()
            .key;
        {
            let mut state = view.selection.0.borrow_mut();
            state.response = 0;
            state.anchor = Some(Position { row: key, byte: 0 });
            state.head = Some(Position { row: key, byte: 5 });
        }
        view.replace_rebased_history(
            PreparedHistory::new(vec![row("Older"), row("First answer")], false),
            1,
            cx,
        );
        assert_eq!(view.selected_text().as_deref(), Some("First"));
    });
}
#[gpui::test]
fn authoritative_snapshot_tails_keep_valid_prefix_selection_and_reading_anchor(
    cx: &mut TestAppContext,
) {
    for bot in [false, true] {
        let view = cx.new(|_| TranscriptView::new());
        view.update(cx, |view, cx| {
            if bot {
                view.bot_mode("bot", 0xffffff, "Bot");
            }
            let rows: Vec<_> = ["First answer", "Second answer", "Last answer"]
                .into_iter()
                .map(|text| Row {
                    role: "assistant".into(),
                    text: text.into(),
                    first: true,
                    sent_at: None,
                })
                .collect();
            view.install_prepared_history(PreparedHistory::new(rows.clone(), bot), cx);
            let item_ix = view
                .rows
                .iter()
                .position(|row| matches!(row.body, Body::Markdown(_)))
                .unwrap();
            let key = view.rows[item_ix].key;
            view.list.pause_following_tail();
            view.list.scroll_to(ListOffset {
                item_ix,
                offset_in_item: px(7.),
            });
            {
                let mut selection = view.selection.0.borrow_mut();
                selection.response = 0;
                selection.anchor = Some(Position { row: key, byte: 0 });
                selection.head = Some(Position { row: key, byte: 5 });
            }
            view.patch_snapshot(1, vec![], 1, cx);
            assert_eq!(view.source, rows[..1]);
            assert_eq!(view.selected_text().as_deref(), Some("First"));
            assert!(view.rows.iter().all(|row| row.key.0 == 0));
            assert!(
                view.motions.is_empty(),
                "removed rows do not linger for an exit animation"
            );
            let restored = view.list.logical_scroll_top();
            assert_eq!(view.rows[restored.item_ix].key, key);
            assert_eq!(restored.offset_in_item, px(7.));
            assert!(!view.list.is_following_tail());
            view.patch(1, rows[1..].to_vec(), cx);
            view.patch_snapshot(
                0,
                vec![Row {
                    text: "Rewritten answer".into(),
                    ..rows[0].clone()
                }],
                1,
                cx,
            );
            assert!(
                view.rows
                    .iter()
                    .any(|row| matches!(&row.body, Body::Markdown(markdown)
                    if crate::markdown::selection_text(markdown) == "Rewritten answer")),
                "a shorter changed tail resets the reused Markdown parser"
            );
            assert!(
                view.selected_text().is_none(),
                "selection in rewritten text is stale"
            );
            assert_eq!(view.source.len(), 1);
            assert!(view.motions.is_empty());
            view.patch_snapshot(0, vec![], 0, cx);
            assert!(view.selected_text().is_none());
            assert!(view.source.is_empty() && view.rows.is_empty() && view.messages.is_empty());
            assert_eq!(view.source_bytes, 0);
            assert_eq!(view.list.item_count(), 1);
            assert_eq!(view.list.logical_scroll_top().item_ix, 0);
        });
    }
}

struct ReaderFixture {
    transcript: gpui::Entity<TranscriptView>,
    show: bool,
}
impl Render for ReaderFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .when(self.show, |d| d.child(self.transcript.clone()))
    }
}

#[gpui::test]
fn resizing_a_long_paragraph_keeps_the_reading_glyph_at_the_same_height(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(gpui::WindowBounds::Windowed(Bounds::new(
                    gpui::point(px(0.), px(0.)),
                    size(px(900.), px(600.)),
                ))),
                ..Default::default()
            },
            |_, cx| {
                let transcript = cx.new(|_| TranscriptView::new());
                cx.new(|_| ReaderFixture {
                    transcript,
                    show: true,
                })
            },
        )
        .unwrap()
    });
    let view = window
        .update(cx, |fixture, _, _| fixture.transcript.clone())
        .unwrap();
    view.update(cx, |view, cx| {
        let text = (0..1500)
            .map(|i| format!("word{i} café "))
            .collect::<String>();
        view.patch(
            0,
            vec![Row {
                role: "assistant".into(),
                text,
                first: true,
                sent_at: None,
            }],
            cx,
        );
        for reveal in view.reveals.values_mut() {
            reveal.settle(Instant::now());
        }
        view.prompt_rise = None;
        view.motions.clear();
        view.rebuild(0, cx);
        view.list.pause_following_tail();
        let item_ix = view
            .rows
            .iter()
            .position(|row| matches!(row.body, Body::Markdown(_)))
            .unwrap();
        view.list.scroll_to(ListOffset {
            item_ix,
            offset_in_item: px(400.),
        });
        cx.notify();
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    let anchor = view.read_with(cx, |view, _| {
        view.selection
            .0
            .borrow()
            .reader
            .clone()
            .expect("a painted reading glyph")
    });
    for (width, hidden) in [
        (500., false),
        (650., false),
        (420., false),
        (880., true),
        (500., false),
    ] {
        if hidden {
            view.update(cx, |view, cx| view.visible(false, cx));
            window
                .update(cx, |fixture, _, cx| {
                    fixture.show = false;
                    cx.notify();
                })
                .unwrap();
            cx.refresh().unwrap();
            cx.run_until_parked();
        }
        cx.simulate_window_resize(window.into(), size(px(width), px(600.)));
        if hidden {
            view.update(cx, |view, cx| view.visible(true, cx));
            window
                .update(cx, |fixture, _, cx| {
                    fixture.show = true;
                    cx.notify();
                })
                .unwrap();
            cx.refresh().unwrap();
        }
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            let state = view.selection.0.borrow();
            let new_y = anchor_y(&state, &anchor).unwrap() - view.viewport.top();
            assert!(
                (new_y - anchor.top).abs() <= px(1.),
                "reading glyph moved from {:?} to {new_y:?} at width {width}",
                anchor.top
            );
            assert_eq!(
                state.reader.as_ref().unwrap().position,
                anchor.position,
                "a divider drag retains one glyph"
            );
        });
    }
    window
        .update(cx, |_, window, cx| {
            window.dispatch_event(
                gpui::PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
                    position: gpui::point(px(250.), px(180.)),
                    delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-160.))),
                    ..Default::default()
                }),
                cx,
            );
        })
        .unwrap();
    cx.run_until_parked();
    let new_reader = view.read_with(cx, |view, _| view.selection.0.borrow().reader.clone());
    assert!(
        new_reader
            .as_ref()
            .is_none_or(|a| a.position != anchor.position),
        "manual scroll supersedes the old bookmark"
    );
}

#[gpui::test]
fn resizing_a_transcript_keeps_the_selected_text_identity(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|_| TranscriptView::new())
        })
        .unwrap()
    });
    window
        .update(cx, |view, _, cx| {
            view.patch(
                0,
                vec![Row {
                    role: "assistant".into(),
                    text: "Keep café selected while this paragraph wraps at another width.".into(),
                    first: true,
                    sent_at: None,
                }],
                cx,
            );
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            let key = view
                .rows
                .iter()
                .find(|row| matches!(&row.body, Body::Markdown(_)))
                .unwrap()
                .key;
            let mut state = view.selection.0.borrow_mut();
            state.response = 0;
            state.anchor = Some(Position { row: key, byte: 5 });
            state.head = Some(Position { row: key, byte: 10 });
            drop(state);
            assert_eq!(view.selected_text().as_deref(), Some("café"));
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    cx.simulate_window_resize(window.into(), gpui::size(px(300.), px(450.)));
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| {
            assert_eq!(view.selected_text().as_deref(), Some("café"))
        })
        .unwrap();
}

#[gpui::test]
fn snapshot_truncation_preserves_surviving_selection_and_clears_deleted_selection(
    cx: &mut TestAppContext,
) {
    let view = cx.new(|_| TranscriptView::new());
    view.update(cx, |view, cx| {
        let rows: Vec<_> = ["First answer", "Second answer"]
            .into_iter()
            .map(|text| Row {
                role: "assistant".into(),
                text: text.into(),
                first: true,
                sent_at: None,
            })
            .collect();
        view.install_prepared_history(PreparedHistory::new(rows.clone(), false), cx);
        let first = view
            .rows
            .iter()
            .find(|row| matches!(&row.body, Body::Markdown(_)))
            .unwrap()
            .key;
        {
            let mut state = view.selection.0.borrow_mut();
            state.response = 0;
            state.anchor = Some(Position {
                row: first,
                byte: 0,
            });
            state.head = Some(Position {
                row: first,
                byte: 5,
            });
        }
        assert_eq!(view.selected_text().as_deref(), Some("First"));
        view.install_prepared_history(PreparedHistory::new(rows[..1].to_vec(), false), cx);
        assert_eq!(view.selected_text().as_deref(), Some("First"));
        view.install_prepared_history(PreparedHistory::new(vec![], false), cx);
        assert!(view.selected_text().is_none());
        assert!(!view.selection.active());
    });
}

#[gpui::test]
fn rendered_selection_crosses_markdown_and_code_without_chrome_or_other_responses(
    cx: &mut TestAppContext,
) {
    let view = cx.new(|_| TranscriptView::new());
    view.update(cx,|view,cx|{
            view.patch(0,vec![Row{role:"assistant".into(),text:"Hello **café**.\n\n```rs\nlet x = 1;\n```\n\nTail".into(),first:true,sent_at:None},Row{role:"user".into(),text:"Next".into(),first:true,sent_at:None},Row{role:"assistant".into(),text:"Other response".into(),first:true,sent_at:None}],cx);
            let first=view.rows.iter().find(|row|matches!(&row.body,Body::Markdown(md) if crate::markdown::selection_text(md)=="Hello café.")).unwrap().key;
            let code=view.rows.iter().find(|row|matches!(&row.body,Body::Markdown(md) if matches!(md.part,Part::Code{..}))).unwrap().key;
            {let mut selection=view.selection.0.borrow_mut();selection.response=0;selection.quote=true;selection.anchor=Some(Position{row:first,byte:6});selection.head=Some(Position{row:code,byte:10});}
            assert_eq!(view.selected_text().as_deref(),Some("café.\nlet x = 1;"));
            {let mut selection=view.selection.0.borrow_mut();let anchor=selection.anchor;selection.anchor=selection.head;selection.head=anchor;}
            assert_eq!(view.selected_text().as_deref(),Some("café.\nlet x = 1;"));
            view.selection.clear();assert!(view.selected_text().is_none());
        });
}
#[test]
fn selection_ranges_are_direction_independent_and_response_scoped() {
    let selection = Selection::default();
    {
        let mut state = selection.0.borrow_mut();
        state.response = 5;
        state.anchor = Some(Position {
            row: (5, 2),
            byte: 8,
        });
        state.head = Some(Position {
            row: (5, 0),
            byte: 3,
        });
    }
    assert_eq!(selection.range((5, 0), 5, 0, 10), 3..10);
    assert_eq!(selection.range((5, 1), 5, 0, 20), 0..20);
    assert_eq!(selection.range((5, 2), 5, 4, 20), 0..4);
    assert_eq!(selection.range((5, 1), 6, 0, 20), 0..0);
}
#[test]
fn unicode_anchors_do_not_split_composed_graphemes() {
    assert_eq!(snap("e\u{301}🙂", 2), 0);
    assert_eq!(snap("e\u{301}🙂", 4), 3);
    assert_eq!(word("hello café", 7), 6..11);
}
