
use super::*;

fn rich_source() -> Vec<Value> {
    vec![
        json!({"id":"prompt","role":"user","text":"Saved prompt","turnModel":{"harness":"codex","id":"saved"},"providerTurnId":"saved-turn"}),
        json!({"id":"tool","role":"tool","text":"Saved tool","tool":{"kind":"execute","title":"cat output","detail":"🙂 café 漢字\n".repeat(20_000),"future":true},"unknown":{"keep":true}}),
        json!({"id":"invisible","role":"assistant","internal":true,"text":"Retained hidden payload"}),
        json!({"id":"answer","role":"assistant","text":"Saved answer"}),
    ]
}
fn fixture(bot: bool) -> (Store, i64, String, Transcript) {
    fixture_at(Path::new(":memory:"), bot)
}
fn fixture_at(path: &Path, bot: bool) -> (Store, i64, String, Transcript) {
    let mut store = Store::open_vorkspace(path).unwrap();
    let id = store.create("codex", "/project").unwrap().id;
    let original = store.persisted_session_id(id).unwrap();
    if bot {
        store
            .conn
            .execute_batch(include_str!("../../../../tests/fixtures/bot-schema.sql"))
            .unwrap();
        store
            .conn
            .execute("INSERT INTO bot_transcripts VALUES(?1)", [&original])
            .unwrap();
    }
    store.shared_store_blocks(id, &rich_source()).unwrap();
    let transcript = store.load(id).unwrap();
    // Prime native normalized storage and its compact projection index.
    store.write_rows(id, transcript.rows.len(), &[]).unwrap();
    (store, id, original, transcript)
}

#[test]
fn first_bookkeeping_writes_preserve_other_active_histories_and_reopen() {
    for bot in [false, true] {
        for (operation, separate_writer) in [
            ("note", false),
            ("note", true),
            ("submission", false),
            ("submission", true),
            ("automation", true),
            ("queue", false),
            ("origin", false),
            ("claim", false),
            ("queue", true),
            ("origin", true),
            ("claim", true),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("history.db");
            let (mut store, id, original, mut history) = fixture_at(&path, bot);
            let other = store.create("codex", "/other").unwrap().id;
            let other_original = store.persisted_session_id(other).unwrap();
            let writer = separate_writer.then(|| Store::open_vorkspace(&path).unwrap());
            let actor = writer.as_ref().unwrap_or(&store);
            let target = actor.resolve_session_id(&other_original).unwrap();
            let before = store.shared_blocks(id).unwrap();
            let schema: i64 = store
                .conn
                .query_row("PRAGMA schema_version", [], |row| row.get(0))
                .unwrap();
            let queued = crate::follow_up::State {
                revision: 1,
                messages: vec![crate::follow_up::Message {
                    id: "queued-once".into(),
                    text: "Keep the unsent prompt".into(),
                    prepared_prompt: Some("Keep its transport context too".into()),
                    ..Default::default()
                }],
                ..Default::default()
            };
            let message = crate::bot::Message {
                id: "claimed-once".into(),
                text: "Pending Bot work".into(),
                prepared_prompt: None,
                peer: None,
                sent_at: None,
                internal: false,
                error: None,
                attachments: vec![],
            };
            let submission = || crate::store::SubmissionInput {
                attempt_id: "reserved-once".into(),
                user_turn_id: "unsent-once".into(),
                session_id: String::new(),
                payload_json: r#"{"text":"Keep this send"}"#.into(),
                user_turn: Some(json!({"id":"unsent-once","role":"user","text":"Keep this send"})),
            };
            match operation {
                "note" => {
                    actor
                        .upsert_note(&crate::notes::NoteUpsert {
                            id: "saved-note".into(),
                            title: "Saved while a chat streams".into(),
                            body: "Keep the note too".into(),
                            ..Default::default()
                        })
                        .unwrap();
                }
                "submission" => assert!(
                    actor
                        .reserve_local_submission(target, submission())
                        .unwrap()
                        .created
                ),
                "automation" => {
                    assert!(crate::automations::Service::open(&path)
                        .unwrap()
                        .list()
                        .unwrap()
                        .is_empty());
                }
                "queue" => actor.accept_follow_ups(target, &queued).unwrap(),
                "origin" => actor
                    .save_setting(
                        &format!("bot-draft-origin:{target}"),
                        r#"{"id":"draft","revision":1}"#,
                    )
                    .unwrap(),
                _ => {
                    let mut roster = crate::bot::Roster::default();
                    roster.bots.push(crate::bot::Bot {
                        id: "bot".into(),
                        outbox: vec![message.clone()],
                        ..Default::default()
                    });
                    roster.save(actor).unwrap();
                    assert!(actor.claim_bot_message("bot", &message, "owner").unwrap());
                }
            }
            let from = history.append(" continued after bookkeeping");
            store.write_rows(id, from, &history.rows[from..]).unwrap();
            assert_eq!(
                store
                    .conn
                    .query_row("PRAGMA schema_version", [], |row| row.get::<_, i64>(0))
                    .unwrap(),
                schema
            );
            assert_eq!(store.shared_blocks(id).unwrap()[..3], before[..3]);
            drop(writer);
            drop(store);
            let reopened = Store::open_vorkspace(&path).unwrap();
            let id = reopened.resolve_session_id(&original).unwrap();
            let other = reopened.resolve_session_id(&other_original).unwrap();
            assert_eq!(reopened.load(id).unwrap().rows, history.rows);
            assert_eq!(reopened.shared_blocks(id).unwrap()[1], before[1]);
            match operation {
                "note" => assert_eq!(
                    reopened.get_note("saved-note").unwrap().unwrap().body,
                    "Keep the note too"
                ),
                "submission" => {
                    let receipt = reopened
                        .reserve_local_submission(other, submission())
                        .unwrap();
                    assert!(
                        !receipt.created,
                        "reopening must not reserve a duplicate send"
                    );
                    assert_eq!(receipt.receipt.user_turn.unwrap()["text"], "Keep this send");
                }
                "automation" => assert!(crate::automations::Service::open(&path)
                    .unwrap()
                    .list()
                    .unwrap()
                    .is_empty()),
                "queue" => assert_eq!(
                    reopened.follow_ups(other).unwrap().messages,
                    queued.messages
                ),
                "origin" => assert_eq!(
                    reopened
                        .setting(&format!("bot-draft-origin:{other}"))
                        .unwrap()
                        .as_deref(),
                    Some(r#"{"id":"draft","revision":1}"#)
                ),
                _ => {
                    assert!(!reopened
                        .claim_bot_message("bot", &message, "other-owner")
                        .unwrap());
                    assert_eq!(
                        crate::bot::Roster::load(&reopened)
                            .unwrap()
                            .get("bot")
                            .unwrap()
                            .outbox
                            .len(),
                        1
                    );
                }
            }
        }
    }
}

#[test]
fn first_bot_draft_acceptance_preserves_active_history_input_and_receipt() {
    for bot in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.db");
        let (store, id, original, history) = fixture_at(&path, bot);
        let before = store.shared_blocks(id).unwrap();
        let input = json!({"cwd":"/project","text":"Unsent Bot draft","selectionStart":4,"selectionEnd":9,"attachments":[],"mcpTags":[{"token":"@mcp/docs","opaque":true,"server":{"provider":"codex","name":"docs","scope":"user","configPath":"/private/config.toml","transport":"stdio","enabled":true}}]});
        let revision = store.save_composer_draft(id, 0, &input).unwrap();
        let mut state = store.follow_ups(id).unwrap();
        state.revision += 1;
        state.draft = Some(crate::follow_up::Message {
            id: "draft-version".into(),
            text: "Unsent Bot draft".into(),
            prepared_prompt: Some("MCP context: docs\n\nUnsent Bot draft".into()),
            at: history.rows.len(),
            ..Default::default()
        });
        store
            .accept_bot_draft(id, &state, revision, "draft-version")
            .unwrap();
        assert!(
            store.shared_blocks(id).unwrap()[..before.len()] == before,
            "accepting a draft must retain every existing source field"
        );
        let mut settings = store.agent_settings(id).unwrap().unwrap_or_default();
        settings.turns.insert(
            1,
            crate::agent::TurnInfo {
                duration_known: true,
                duration_ms: 1200,
                ..Default::default()
            },
        );
        store.save_agent_settings(id, &settings).unwrap();
        drop(store);
        let reopened = Store::open_vorkspace(&path).unwrap();
        let id = reopened.resolve_session_id(&original).unwrap();
        let draft = reopened.follow_ups(id).unwrap().draft.unwrap();
        assert_eq!(draft.id, "draft-version");
        assert_eq!(draft.prepared_prompt, state.draft.unwrap().prepared_prompt);
        assert_eq!(reopened.composer_draft(id).unwrap().unwrap().payload, input);
        assert_eq!(
            reopened.agent_settings(id).unwrap().unwrap().turns[&1].duration_ms,
            1200
        );
        assert_eq!(reopened.shared_blocks(id).unwrap()[1], before[1]);
        let marker: Value = serde_json::from_str(
            &reopened
                .setting(&format!("bot-draft-origin:{id}"))
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(marker, json!({"id":"draft-version","revision":revision}));
    }
}

#[test]
fn tail_saves_skip_rich_prefix_and_match_full_merge_across_appends_and_earlier_edits() {
    for bot in [false, true] {
        let (mut store, id, _, mut transcript) = fixture(bot);
        for operation in 0..8 {
            let mut reference = store.shared_blocks(id).unwrap();
            let start = match operation {
                0 | 1 | 4 => transcript.append(" continued 🙂"),
                2 => {
                    let at = transcript.begin("user");
                    transcript.append("Next prompt");
                    at
                }
                3 => {
                    let at = transcript.begin("assistant");
                    transcript.append("Next answer");
                    at
                }
                5 => {
                    transcript.rows[0].text.push_str(" revised");
                    0
                }
                6 => {
                    let at = transcript.begin("assistant");
                    transcript.append("a".repeat(crate::transcript::ROW_BYTES + 10).as_str());
                    at
                }
                _ => transcript.append(" final"),
            };
            reference = blocks::merge(&reference, start, &transcript.rows[start..]).unwrap();
            HISTORY_READ_BYTES.with(|count| count.set(0));
            HISTORY_ENCODE_BYTES.with(|count| count.set(0));
            store
                .write_rows(id, start, &transcript.rows[start..])
                .unwrap();
            let read = HISTORY_READ_BYTES.with(|count| count.get());
            let encoded = HISTORY_ENCODE_BYTES.with(|count| count.get());
            if operation != 5 {
                assert!(
                    encoded < 16_000,
                    "An unchanged rich prefix was serialized: {encoded} bytes"
                );
                assert!(
                    read < 16_000,
                    "An unchanged rich prefix was read: {read} bytes"
                );
            }
            let stored = store.shared_blocks(id).unwrap();
            // Appended block IDs are generated independently by the
            // straightforward reference; compare all other source fields.
            for (actual, expected) in stored.iter().zip(&reference) {
                let mut actual = actual.clone();
                let mut expected = expected.clone();
                actual.as_object_mut().unwrap().remove("id");
                expected.as_object_mut().unwrap().remove("id");
                assert_eq!(actual, expected);
            }
            assert_eq!(stored.len(), reference.len());
            assert_eq!(blocks::project(&stored).unwrap(), transcript.rows);
            assert_eq!(
                stored[1],
                rich_source()[1],
                "Bounded tool views must not replace durable output"
            );
        }
    }
}

#[test]
fn scalar_guards_reject_external_raw_and_normalized_edits_but_allow_metadata() {
    for mutation in ["raw", "edit", "delete", "marker"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.db");
        let mut store = Store::open_vorkspace(&path).unwrap();
        let id = store.create("codex", "/project").unwrap().id;
        let original = store.persisted_session_id(id).unwrap();
        store.shared_store_blocks(id, &rich_source()).unwrap();
        let mut transcript = store.load(id).unwrap();
        store.write_rows(id, transcript.rows.len(), &[]).unwrap();
        let external = Connection::open(&path).unwrap();
        external
            .execute(
                "UPDATE sessions SET title='Renamed',context_used=12 WHERE id=?1",
                [&original],
            )
            .unwrap();
        let from = transcript.append(" after rename");
        store
            .write_rows(id, from, &transcript.rows[from..])
            .unwrap();
        match mutation {
            "raw" => {
                external.execute("UPDATE sessions SET blocks_json=?2 WHERE id=?1",params![original,json!([{"id":"replacement","role":"assistant","text":"External replacement"}]).to_string()]).unwrap();
            }
            "edit" => {
                external.execute("UPDATE ordinary_blocks SET block_json=json_set(block_json,'$.unknown.external',true) WHERE session_id=?1 AND position=0",[&original]).unwrap();
            }
            "delete" => {
                external
                    .execute(
                        "DELETE FROM ordinary_blocks WHERE session_id=?1 AND position=1",
                        [&original],
                    )
                    .unwrap();
            }
            _ => {
                external
                    .execute(
                        "UPDATE ordinary_transcripts SET normalized=0 WHERE session_id=?1",
                        [&original],
                    )
                    .unwrap();
            }
        }
        let saved: Vec<Value> = store.shared_blocks(id).unwrap();
        assert!(store
            .write_rows(id, from, &transcript.rows[from..])
            .unwrap_err()
            .contains("changed in another window"));
        assert_eq!(store.shared_blocks(id).unwrap(), saved);
        drop(store);
        drop(external);
        let reopened = Store::read_vorkspace(&path).unwrap();
        let id = reopened.resolve_session_id(&original).unwrap();
        assert_eq!(reopened.shared_blocks(id).unwrap(), saved);
        assert_eq!(
            reopened.load(id).unwrap().rows,
            blocks::project(&saved).unwrap()
        );
    }
}

#[test]
fn failed_tail_transaction_keeps_the_prior_guard_and_all_durable_rows_for_retry() {
    let (mut store, id, original, mut transcript) = fixture(false);
    let before = store.shared_blocks(id).unwrap();
    let revision = store.history_revision(&original).unwrap();
    // Install a deterministic failure before priming the schema cache, so
    // this exercises transaction rollback rather than schema invalidation.
    store.conn.execute_batch("CREATE TEMP TRIGGER reject_tail BEFORE UPDATE ON ordinary_blocks WHEN NEW.block_id='answer' BEGIN SELECT RAISE(ABORT,'injected failure'); END").unwrap();
    transcript.rows[0].text.push_str(" changed");
    transcript
        .rows
        .last_mut()
        .unwrap()
        .text
        .push_str(" changed");
    assert!(store
        .write_rows(id, 0, &transcript.rows)
        .unwrap_err()
        .contains("injected failure"));
    assert_eq!(store.history_revision(&original).unwrap(), revision);
    assert_eq!(store.shared_blocks(id).unwrap(), before);
    store
        .conn
        .execute_batch("DROP TRIGGER reject_tail")
        .unwrap();
    store.write_rows(id, 0, &transcript.rows).unwrap();
    assert_eq!(store.load(id).unwrap().rows, transcript.rows);
    store.delete(id).unwrap();
    assert_eq!(
        store
            .conn
            .query_row("SELECT count(*) FROM session_history_revisions", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
}

#[test]
fn normalization_reopen_and_accepted_receipt_keep_unloaded_tool_payload_and_queue_identity() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history.db");
    let mut store = Store::open_vorkspace(&path).unwrap();
    let id = store.create("codex", "/project").unwrap().id;
    let original = store.persisted_session_id(id).unwrap();
    store.shared_store_blocks(id, &rich_source()).unwrap();
    let mut transcript = store.load(id).unwrap();
    let at = transcript.append(" first save");
    store.write_rows(id, at, &transcript.rows[at..]).unwrap();
    store
        .save_follow_ups(
            id,
            &crate::follow_up::State {
                revision: 1,
                messages: vec![crate::follow_up::Message {
                    id: "queued".into(),
                    text: "Continue".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .unwrap();
    let at = transcript.begin("user");
    transcript.append("Continue");
    store
        .write_rows_with_message(id, at, &transcript.rows[at..], Some("queued"))
        .unwrap();
    let at = transcript.begin("assistant");
    transcript.append("Reply");
    store.write_rows(id, at, &transcript.rows[at..]).unwrap();
    drop(store);
    let mut reopened = Store::open_vorkspace(&path).unwrap();
    let id = reopened.resolve_session_id(&original).unwrap();
    assert_eq!(reopened.load(id).unwrap().rows, transcript.rows);
    assert!(reopened.follow_ups(id).unwrap().messages.is_empty());
    assert_eq!(
        reopened
            .setting(&format!("bot-delivered:{id}:queued"))
            .unwrap(),
        Some("1".into())
    );
    assert_eq!(reopened.shared_blocks(id).unwrap()[1], rich_source()[1]);
    let at = transcript.append(" after reopen");
    reopened.write_rows(id, at, &transcript.rows[at..]).unwrap();
    let at = transcript.append(" incremental again");
    HISTORY_READ_BYTES.with(|count| count.set(0));
    reopened.write_rows(id, at, &transcript.rows[at..]).unwrap();
    assert!(HISTORY_READ_BYTES.with(|count| count.get()) < 16_000);
}
#[test]
fn explicit_empty_or_malformed_blob_replacement_never_resurrects_normalized_history() {
    for replacement in ["[]", "not JSON"] {
        let (mut store, id, original, transcript) = fixture(false);
        // The normalized cache was already [], so explicit clearing must
        // invalidate even when OLD.blocks_json equals NEW.blocks_json.
        store
            .conn
            .execute(
                "UPDATE sessions SET blocks_json=?2 WHERE id=?1",
                params![original, replacement],
            )
            .unwrap();
        assert!(store
            .write_rows(id, transcript.rows.len(), &[])
            .unwrap_err()
            .contains("changed in another window"));
        if replacement == "[]" {
            assert!(store.load(id).unwrap().rows.is_empty());
        } else {
            assert!(store.load(id).is_err());
        }
    }
}

#[test]
fn first_normalization_failure_retains_the_complete_legacy_source_for_retry() {
    let mut store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
    store
        .conn
        .execute_batch(include_str!(
            "../../../../tests/fixtures/ordinary-schema.sql"
        ))
        .unwrap();
    let id = store.create("codex", "/project").unwrap().id;
    let original = store.persisted_session_id(id).unwrap();
    let before = rich_source();
    store.shared_store_blocks(id, &before).unwrap();
    let transcript = store.load(id).unwrap();
    store.conn.execute_batch("CREATE TEMP TRIGGER reject_normalization BEFORE INSERT ON ordinary_blocks WHEN NEW.block_id='answer' BEGIN SELECT RAISE(ABORT,'injected normalization failure'); END").unwrap();
    assert!(store
        .write_rows(id, transcript.rows.len(), &[])
        .unwrap_err()
        .contains("injected normalization failure"));
    assert_eq!(store.shared_blocks(id).unwrap(), before);
    assert!(!store.shared_ordinary_indexed(&original).unwrap());
    assert_eq!(
        store
            .conn
            .query_row("SELECT count(*) FROM ordinary_blocks", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    store
        .conn
        .execute_batch("DROP TRIGGER reject_normalization")
        .unwrap();
    store.write_rows(id, transcript.rows.len(), &[]).unwrap();
    assert_eq!(store.shared_blocks(id).unwrap(), before);
    assert!(store.shared_ordinary_indexed(&original).unwrap());
}

#[test]
fn normalized_block_ownership_moves_are_rejected_for_both_stores() {
    for bot in [false, true] {
        let (store, _, original, _) = fixture(bot);
        let other = store.create("codex", "/project").unwrap();
        let other = store.persisted_session_id(other.id).unwrap();
        let table = if bot { "bot_blocks" } else { "ordinary_blocks" };
        let error = store
            .conn
            .execute(
                &format!("UPDATE {table} SET session_id=?2 WHERE session_id=?1"),
                params![original, other],
            )
            .unwrap_err();
        assert!(error.to_string().contains("ownership cannot be changed"));
    }
}

#[test]
fn malformed_duplicate_positions_decline_incremental_indexing() {
    let (store, id, original, _) = fixture(false);
    store
        .conn
        .execute(
            "UPDATE ordinary_blocks SET position=0 WHERE session_id=?1",
            [&original],
        )
        .unwrap();
    let blocks = store.shared_blocks(id).unwrap();
    let error = store.shared_build_history_index(id, &blocks).unwrap_err();
    assert!(error.contains("positions are not unique"));
    assert_eq!(store.shared_blocks(id).unwrap(), blocks);
}
#[test]
fn schema_change_requires_each_stale_writer_to_reload_and_allows_recovery_without_restart() {
    let (mut store, first, _, first_history) = fixture(false);
    let second = store.create("codex", "/project").unwrap().id;
    let history = rich_source();
    store.shared_store_blocks(second, &history).unwrap();
    let second_history = store.load(second).unwrap();
    store
        .write_rows(second, second_history.rows.len(), &[])
        .unwrap();
    store
        .conn
        .execute_batch("CREATE TABLE unrelated_external_schema_change(value TEXT)")
        .unwrap();
    assert!(store
        .save_follow_ups(first, &Default::default())
        .unwrap_err()
        .contains("changed in another window"));
    assert!(store
        .write_rows(first, first_history.rows.len(), &[])
        .unwrap_err()
        .contains("changed in another window"));
    store.load(first).unwrap();
    store
        .write_rows(first, first_history.rows.len(), &[])
        .unwrap();
    assert!(store
        .write_rows(second, second_history.rows.len(), &[])
        .unwrap_err()
        .contains("changed in another window"));
    store.load(second).unwrap();
    store
        .write_rows(second, second_history.rows.len(), &[])
        .unwrap();
}

#[test]
fn context_owner_predicate_skips_history_and_rechecks_provider_and_account_at_storage() {
    for shared in [false, true] {
        let store = if shared {
            Store::open_vorkspace(Path::new(":memory:")).unwrap()
        } else {
            Store::open(Path::new(":memory:")).unwrap()
        };
        let id = store.create("codex", "/project").unwrap().id;
        let mut settings = Settings::default();
        settings.account.id = "selected-account".into();
        store.save_agent_settings(id, &settings).unwrap();
        if shared {
            store
                .conn
                .execute(
                    "UPDATE sessions SET blocks_json='unavailable history' WHERE id=?1",
                    [store.persisted_session_id(id).unwrap()],
                )
                .unwrap();
        }
        let current = crate::context::Usage {
            used: 12,
            window: Some(100),
        };
        assert!(store
            .save_context_usage_if_owner(id, &current, "codex", "selected-account")
            .unwrap());
        let superseded = crate::context::Usage {
            used: 99,
            window: Some(100),
        };
        assert!(!store
            .save_context_usage_if_owner(id, &superseded, "claude", "selected-account")
            .unwrap());
        assert!(!store
            .save_context_usage_if_owner(id, &superseded, "codex", "old-account")
            .unwrap());
        assert_eq!(store.context_usage(id).unwrap().unwrap().used, 12);
    }
}
#[test]
fn normalized_legacy_schema_keeps_scalar_presence_without_history_payload_reads() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("old.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(include_str!("../../../../tests/fixtures/legacy-schema.sql"))
        .unwrap();
    drop(conn);
    let mut store = Store::open_vorkspace(&path).unwrap();
    let id = store.create("codex", "/project").unwrap().id;
    let original = store.persisted_session_id(id).unwrap();
    store.shared_store_blocks(id, &rich_source()).unwrap();
    let transcript = store.load(id).unwrap();
    store.write_rows(id, transcript.rows.len(), &[]).unwrap();
    assert_eq!(
        store
            .conn
            .query_row(
                "SELECT blocks_json FROM sessions WHERE id=?1",
                [&original],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "[]"
    );
    HISTORY_READ_BYTES.with(|count| count.set(0));
    assert!(store.sessions().unwrap()[0].has_messages);
    assert!(store.bot_session(id).unwrap().unwrap().has_messages);
    // Preference reads use the same scalar helper and must not seed a
    // blank-session default over a conversation that actually has history.
    store.shared_agent_preferences(id).unwrap();
    assert!(HISTORY_READ_BYTES.with(|count| count.get()) < 16_000);
}
