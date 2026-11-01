
use super::*;
use gpui::TestAppContext;

#[gpui::test]
fn indexed_bot_pages_reveal_saved_global_rows_and_keep_scroll_anchor(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    let store = vorkspace_core::store::Store::open_vorkspace(&path).unwrap();
    let id = store.create("codex", "/project").unwrap().id;
    let original = store.persisted_session_id(id).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(include_str!(
        "../../../../vorkspace-core/tests/fixtures/bot-schema.sql"
    ))
    .unwrap();
    conn.execute("INSERT INTO bot_transcripts VALUES(?1)", [&original])
        .unwrap();
    for i in 0..70 {
        let role = if i % 2 == 0 { "user" } else { "assistant" };
        let block =
            serde_json::json!({"id":format!("block-{i}"),"role":role,"text":format!("needle {i}")});
        conn.execute(
            "INSERT INTO bot_blocks VALUES(?1,?2,?3,?4,'',?5,0)",
            rusqlite::params![
                original,
                i + 1,
                format!("block-{i}"),
                block.to_string(),
                i % 2 == 0
            ],
        )
        .unwrap();
        if i % 2 == 0 {
            conn.execute(
                "INSERT INTO bot_turns VALUES(?1,?2)",
                rusqlite::params![original, i + 1],
            )
            .unwrap();
        }
    }
    let mut page = store.indexed_history_page(id, None, 10).unwrap().unwrap();
    assert_eq!(page.global_start, 50);
    let prepared = PreparedHistory::new(std::mem::take(&mut page.rows), false);
    let store = std::sync::Arc::new(std::sync::Mutex::new(store));
    let view = cx.new(|_| TranscriptView::new());
    let key = view.update(cx, |view, cx| {
        view.bot_mode("bot", 0xffffff, "Bot");
        view.install_storage_history(page, prepared, store.clone(), id, cx);
        assert_eq!(view.source.len(), 20);
        assert_eq!(view.storage_origin(), Some(50));
        view.list.pause_following_tail();
        view.list.scroll_to(ListOffset {
            item_ix: 4,
            offset_in_item: px(7.),
        });
        let key = view.rows[4].key;
        view.expanded.insert(2);
        view.expanded_prompts.insert(2);
        key
    });
    let poison = store.clone();
    let _ = std::thread::spawn(move || {
        let _guard = poison.lock().unwrap();
        panic!("simulate a temporarily unavailable store");
    })
    .join();
    view.update(cx, |view, cx| view.load_older_history(cx));
    cx.run_until_parked();
    view.update(cx, |view, _| {
        assert_eq!(view.storage_origin(), Some(50));
        assert!(view.storage_history.as_ref().unwrap().error.is_some());
        assert!(view.storage_has_older());
    });
    store.clear_poison();
    view.update(cx, |view, cx| {
        view.load_older_history(cx);
        view.load_older_history(cx);
    });
    cx.run_until_parked();
    view.update(cx, |view, cx| {
        assert_eq!(view.storage_origin(), Some(30));
        assert_eq!(view.source.len(), 40);
        assert!(view.expanded.contains(&22));
        assert!(view.expanded_prompts.contains(&22));
        let offset = view.list.logical_scroll_top();
        assert_eq!(view.rows[offset.item_ix].key, (key.0 + 20, key.1));
        assert_eq!(offset.offset_in_item, px(7.));
        view.reveal_storage_search(1, "needle", cx);
    });
    cx.run_until_parked();
    view.update(cx, |view, _| {
        assert_eq!(view.storage_origin(), Some(0));
        assert_eq!(view.search_match, Some((1, "needle".into())));
        assert_eq!(view.source.len(), 70);
    });
}

fn long_bot(turns: usize) -> Vec<Row> {
    (0..turns).flat_map(|turn| [
            Row { role: "user".into(), text: format!("Question {turn}"), first: true, sent_at: Some(turn as u64) },
            Row { role: "assistant".into(), text: format!("Answer **{turn}**.\n\nA second paragraph with [a link](https://example.com)."), first: true, sent_at: None },
        ]).collect()
}

#[gpui::test]
fn snapshot_tail_shorter_than_the_bot_page_reveals_the_retained_prefix(cx: &mut TestAppContext) {
    let view = cx.new(|_| TranscriptView::new());
    view.update(cx, |view, cx| {
        view.bot_mode("bot", 0xffffff, "Bot");
        let rows = long_bot(25);
        view.install_prepared_history(PreparedHistory::new(rows.clone(), true), cx);
        assert_eq!(view.history_start, 30);
        view.patch_snapshot(2, vec![], 2, cx);
        assert_eq!(view.source, rows[..2]);
        assert_eq!(view.history_start, 0);
        assert!(
            view.rows
                .iter()
                .any(|row| matches!(&row.body, Body::Markdown(markdown)
                if crate::markdown::selection_text(markdown).contains("Answer 0.")))
        );
        assert!(view.rows.iter().all(|row| row.key.0 < 2));
    });
}

#[gpui::test]
fn shorter_prepared_snapshot_removes_tail_and_keeps_open_page_and_reading_anchor(
    cx: &mut TestAppContext,
) {
    let view = cx.new(|_| TranscriptView::new());
    view.update(cx, |view, cx| {
        view.bot_mode("bot", 0xffffff, "Bot");
        view.install_prepared_history(PreparedHistory::new(long_bot(25), true), cx);
        view.load_older_history(cx);
    });
    cx.run_until_parked();
    view.update(cx, |view, cx| {
        assert_eq!(view.history_start, 10);
        view.list.pause_following_tail();
        let item_ix = view.rows.iter().position(|row| row.key.0 == 11).unwrap();
        view.list.scroll_to(ListOffset {
            item_ix,
            offset_in_item: px(7.),
        });
        let key = view.rows[item_ix].key;
        let mut replacement = long_bot(20);
        // This lies before the newly prepared recent page, so its reused
        // parser must accept the replacement source rather than retain
        // the old Markdown tree.
        replacement[11].text = "Changed **older** answer.".into();
        view.install_prepared_history(PreparedHistory::new(replacement.clone(), true), cx);
        assert_eq!(view.source, replacement);
        assert_eq!(view.history_start, 10, "the opened older page stays open");
        assert_eq!(view.messages[0].start, 10);
        assert!(view.rows.iter().any(|row| row.key.0 == 11
            && matches!(&row.body, Body::Markdown(markdown)
                    if crate::markdown::selection_text(markdown) == "Changed older answer.")));
        assert!(
            view.rows.iter().all(|row| row.key.0 < 40),
            "deleted rows cannot linger for exit animations"
        );
        assert!(view.motions.is_empty());
        assert!(!view.list.is_following_tail());
        let restored = view.list.logical_scroll_top();
        assert_eq!(view.rows[restored.item_ix].key, key);
        assert_eq!(restored.offset_in_item, px(7.));
    });
}

#[gpui::test]
fn empty_prepared_snapshot_clears_source_rows_and_stale_interaction_state(cx: &mut TestAppContext) {
    let view = cx.new(|_| TranscriptView::new());
    view.update(cx, |view, cx| {
        view.install_prepared_history(PreparedHistory::new(long_bot(2), false), cx);
        view.turns.insert(1, Default::default());
        view.expanded.insert(1);
        view.list.pause_following_tail();
        view.list.scroll_to(ListOffset {
            item_ix: view.rows.len() - 1,
            offset_in_item: px(3.),
        });
        view.install_prepared_history(PreparedHistory::new(vec![], false), cx);
        assert!(view.source.is_empty());
        assert_eq!(view.source_bytes, 0);
        assert!(view.messages.is_empty());
        assert!(view.rows.is_empty());
        assert!(view.turns.is_empty());
        assert!(view.expanded.is_empty());
        assert!(view.motions.is_empty());
        assert_eq!(view.list.item_count(), 1, "only the empty footer remains");
        assert_eq!(view.list.logical_scroll_top().item_ix, 0);
        assert!(!view.list.is_following_tail());
    });
}

#[gpui::test]
fn long_bot_initial_display_and_live_patches_keep_history_unmaterialized(cx: &mut TestAppContext) {
    let source = long_bot(5_000);
    let prepared = PreparedHistory::new(source, true);
    assert_eq!(prepared.parsers.len(), PAGE_TURNS);
    let view = cx.new(|_| TranscriptView::new());
    view.update(cx, |view, cx| {
        view.bot_mode("bot", 0xffffff, "Bot");
        view.install_prepared_history(prepared, cx);
        assert_eq!(view.source.len(), 10_000);
        assert_eq!(view.messages.len(), PAGE_TURNS * 2);
        assert!(
            view.rows.len() < 100,
            "initial layout must depend on the recent page"
        );
        assert_eq!(view.messages[0].start, 9_980);
        assert_eq!(
            view.source[0].text, "Question 0",
            "the archived source remains available"
        );
        view.streaming = true;
        let mut next = long_bot(1);
        next[0].text = "New question".into();
        next[1].text = "New answer".into();
        view.patch(10_000, next, cx);
        assert_eq!(view.source.len(), 10_002);
        assert!(view.messages.len() <= (PAGE_TURNS + 1) * 2);
        assert_eq!(view.messages.last().unwrap().start, 10_001);
        assert_eq!(view.source[0].text, "Question 0");
    });
}

#[gpui::test]
fn internal_bot_reports_also_keep_the_initial_display_bounded(cx: &mut TestAppContext) {
    let mut rows = long_bot(1_000);
    for row in &mut rows {
        if row.is_prompt() {
            row.role = "bot-system".into();
        }
    }
    let view = cx.new(|_| TranscriptView::new());
    view.update(cx, |view, cx| {
        view.bot_mode("bot", 0xffffff, "Bot");
        view.install_prepared_history(PreparedHistory::new(rows, true), cx);
        assert_eq!(view.messages.len(), PAGE_TURNS * 2);
        assert_eq!(view.messages[0].start, 1_980);
        assert!(view.rows.len() < 100);
        assert!(
            view.rows
                .iter()
                .any(|row| matches!(row.body, Body::WorkLine { .. }))
        );
        assert_eq!(view.source.len(), 2_000);
    });
}

#[gpui::test]
fn bot_group_cache_follows_changed_prompt_boundaries(cx: &mut TestAppContext) {
    let view = cx.new(|_| TranscriptView::new());
    view.update(cx, |view, cx| {
        view.bot_mode("bot", 0xffffff, "Bot");
        let mut rows = long_bot(2);
        rows[2].role = "bot-system".into();
        view.patch(0, rows, cx);
        assert_eq!(view.bot_group_start(3), 1);
        let mut prompt = view.source[2].clone();
        prompt.role = "user".into();
        view.patch(2, vec![prompt], cx);
        assert_eq!(view.bot_group_start(3), 3);
    });
}

#[gpui::test]
fn older_bot_pages_preserve_the_reading_anchor_and_search_reaches_archived_rows(
    cx: &mut TestAppContext,
) {
    let view = cx.new(|_| TranscriptView::new());
    let (key, offset) = view.update(cx, |view, cx| {
        view.bot_mode("bot", 0xffffff, "Bot");
        view.install_prepared_history(PreparedHistory::new(long_bot(35), true), cx);
        view.list.pause_following_tail();
        let offset = ListOffset {
            item_ix: 4,
            offset_in_item: px(7.),
        };
        view.list.scroll_to(offset);
        let key = view.rows[4].key;
        view.load_older_history(cx);
        view.load_older_history(cx);
        (key, offset.offset_in_item)
    });
    cx.run_until_parked();
    view.update(cx, |view, cx| {
        assert!(!view.history_loading);
        assert_eq!(
            view.messages[0].start, 30,
            "one request adds ten earlier turns"
        );
        let restored = view.list.logical_scroll_top();
        assert_eq!(view.rows[restored.item_ix].key, key);
        assert_eq!(restored.offset_in_item, offset);
        view.reveal_search(1, "Answer", cx);
    });
    cx.run_until_parked();
    view.update(cx, |view, _| {
        assert_eq!(view.search_match, Some((1, "Answer".into())));
        assert!(view.messages.iter().any(|message| message.start == 1));
        assert_eq!(view.source.len(), 70);
    });
}

#[gpui::test]
fn replacing_history_discards_an_in_flight_bot_page(cx: &mut TestAppContext) {
    let view = cx.new(|_| TranscriptView::new());
    view.update(cx, |view, cx| {
        view.bot_mode("bot", 0xffffff, "Bot");
        view.install_prepared_history(PreparedHistory::new(long_bot(25), true), cx);
        view.load_older_history(cx);
        view.replace_history(long_bot(1), cx);
    });
    cx.run_until_parked();
    view.update(cx, |view, _| {
        assert!(!view.history_loading);
        assert_eq!(view.source.len(), 2);
        assert_eq!(view.messages.len(), 2);
        assert_eq!(view.history_start, 0);
    });
    view.update(cx, |view, cx| view.replace_history(Vec::new(), cx));
    view.update(cx, |view, _| {
        assert!(view.source.is_empty() && view.messages.is_empty());
        assert_eq!(view.history_start, 0);
    });
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            role: "user".into(),
            text: "Find the needle".into(),
            first: true,
            sent_at: Some(7),
        },
        Row {
            role: "assistant".into(),
            text: "needle answer\n\nsecond paragraph".into(),
            first: true,
            sent_at: Some(8),
        },
    ]
}

#[gpui::test]
fn suspended_history_releases_payload_and_restores_search_folds_and_scroll(
    cx: &mut TestAppContext,
) {
    let view = cx.new(|_| TranscriptView::new());
    view.update(cx, |view, cx| {
        view.visible(false, cx);
        let original = rows();
        view.patch(0, original.clone(), cx);
        view.reveal_search(1, "needle", cx);
        view.phases.insert(1, true);
        let offset = view.list.logical_scroll_top();
        let key = view.rows.get(offset.item_ix).unwrap().key;
        assert!(view.history_weight() > 0);
        let epoch = view.highlight_epoch;
        assert!(view.suspend_history(cx));
        assert!(view.source.is_empty() && view.rows.is_empty() && view.messages.is_empty());
        assert_eq!(view.history_weight(), 0);
        assert_ne!(view.highlight_epoch, epoch);
        assert!(!view.suspend_history(cx));
        view.restore_history(original.clone(), cx);
        assert_eq!(view.source.len(), original.len());
        assert_eq!(view.source[0].sent_at, Some(7));
        assert_eq!(view.search_match, Some((1, "needle".into())));
        assert!(view.expanded.contains(&1));
        assert_eq!(view.phases.get(&1), Some(&true));
        assert!(!view.list.is_following_tail());
        let restored = view.list.logical_scroll_top();
        assert_eq!(view.rows[restored.item_ix].key, key);
        assert_eq!(restored.offset_in_item, offset.offset_in_item);
    });
}

#[gpui::test]
fn visible_running_and_unsaved_action_histories_are_pinned(cx: &mut TestAppContext) {
    let view = cx.new(|_| TranscriptView::new());
    view.update(cx, |view, cx| {
        view.patch(0, rows(), cx);
        assert!(!view.suspend_history(cx));
        view.visible(false, cx);
        view.streaming = true;
        assert!(!view.suspend_history(cx));
        view.streaming = false;
        view.note_pending.insert(1);
        assert!(!view.suspend_history(cx));
        view.note_pending.clear();
        assert!(view.suspend_history(cx));
        view.restore_history(Vec::new(), cx);
        assert!(view.source.is_empty());
        assert!(view.history_position.is_none());
    });
}
