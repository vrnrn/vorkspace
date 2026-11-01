use super::*;

fn limit_switch(cx: &mut gpui::TestAppContext, carry: bool, cold: bool) {
    use vorkspace_core::{accounts::Profile, transcript::Row};
    let (window, _, id) = crate::shell::tests::window(cx);
    let (source_id, target, destination, original_count, active, tab_count) = window
        .update(cx, |shell, _, cx| {
            let center = crate::accounts::get(cx).unwrap();
            let (source, target) = center.update(cx, |center, _| {
                (
                    center.book.add(Kind::Claude, "Limited").unwrap(),
                    center.book.add(Kind::Claude, "Available").unwrap(),
                )
            });
            let root = center.read(cx).root.clone();
            let source = Profile::new(&source.id, &root);
            let source_id = source.id.clone();
            let destination = Profile::new(&target.id, &root)
                .home(Kind::Claude)
                .unwrap()
                .join("projects/-fixture/saved-thread.jsonl");
            if carry {
                let transcript = source
                    .home(Kind::Claude)
                    .unwrap()
                    .join("projects/-fixture/saved-thread.jsonl");
                std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
                std::fs::write(transcript, "{\"conversation\":\"original\"}\n").unwrap();
            }
            let rows = vec![
                Row {
                    role: "user".into(),
                    text: "Finish the integration".into(),
                    first: true,
                    sent_at: None,
                },
                Row {
                    role: "assistant".into(),
                    text: "The integration is partly implemented".into(),
                    first: true,
                    sent_at: None,
                },
                // Older native saves persisted this as a system notice and
                // have no structured usage_limit setting.
                Row {
                    role: "system".into(),
                    text: "rate_limit".into(),
                    first: true,
                    sent_at: None,
                },
            ];
            {
                let mut store = locked(&shell.store).unwrap();
                store.save_provider_accounts(&center.read(cx).book).unwrap();
                store.provider_id(id, "saved-thread").unwrap();
                store.write_rows(id, 0, &rows).unwrap();
            }
            shell.sessions[&id].update(cx, |view, cx| {
                let mut settings = view.settings.clone();
                settings.account = source;
                // Block provider launch at preflight: these tests exercise the
                // account transfer and routing without making a live request.
                settings.worktree_removed = true;
                // A restored conversation already has an authoritative saved
                // account; transfer failure must preserve that same record.
                locked(&shell.store)
                    .unwrap()
                    .save_agent_settings(id, &settings)
                    .unwrap();
                view.install_transfer(settings, None, cx);
                view.summary.has_messages = true;
                view.summary.provider_id = Some("saved-thread".into());
                view.transcript.update(cx, |v, cx| {
                    if cold {
                        v.visible(false, cx);
                    }
                    v.patch(0, rows, cx);
                });
                assert!(view.settings.usage_limit.is_none());
                if cold {
                    assert!(view.unload_history(cx));
                }
            });
            let state = (
                source_id,
                target.id.clone(),
                destination,
                shell.sessions.len(),
                shell.workspace.active,
                shell.workspace.tabs.len(),
            );
            center.update(cx, |center, cx| {
                center.choose(
                    Kind::Claude,
                    target.id,
                    "/work/vorkspace".into(),
                    Some(id),
                    cx,
                );
            });
            state
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert_eq!(shell.sessions.len(), original_count);
            assert_eq!(
                locked(&shell.store).unwrap().sessions().unwrap().len(),
                original_count
            );
            assert_eq!(shell.workspace.active, active);
            assert_eq!(shell.workspace.tabs.len(), tab_count);
            let view = shell.sessions[&id].read(cx);
            assert_eq!(view.summary.id, id);
            assert!(
                view.transcript
                    .read(cx)
                    .source_rows()
                    .iter()
                    .any(|row| row.text == "The integration is partly implemented")
            );
            if carry {
                assert_eq!(view.settings.account.id, target);
                assert_eq!(view.summary.provider_id.as_deref(), Some("saved-thread"));
                assert!(view.settings.pending_provider_switch.is_none());
                assert!(view.settings.handoff.is_none());
                assert_eq!(
                    std::fs::read_to_string(&destination).unwrap(),
                    "{\"conversation\":\"original\"}\n"
                );
            } else {
                assert_eq!(view.settings.account.id, source_id);
                assert_eq!(view.summary.provider_id.as_deref(), Some("saved-thread"));
                assert!(view.settings.pending_provider_switch.is_none());
                assert!(view.settings.handoff.is_none());
                assert!(view.account_recovery_limit(cx).is_some());
                let store = locked(&shell.store).unwrap();
                assert_eq!(
                    store.agent_settings(id).unwrap().unwrap().account.id,
                    source_id
                );
                assert_eq!(
                    store
                        .sessions()
                        .unwrap()
                        .into_iter()
                        .find(|s| s.id == id)
                        .unwrap()
                        .provider_id
                        .as_deref(),
                    Some("saved-thread")
                );
                assert!(!destination.exists());
            }
        })
        .unwrap();
}

#[gpui::test]
fn footer_account_switch_carries_a_limited_session_in_place(cx: &mut gpui::TestAppContext) {
    limit_switch(cx, true, false);
}

#[gpui::test]
fn footer_account_switch_restores_a_saved_limit_before_choosing_a_session(
    cx: &mut gpui::TestAppContext,
) {
    limit_switch(cx, true, true);
}

#[gpui::test]
fn footer_account_switch_preserves_original_resume_when_native_history_is_missing(
    cx: &mut gpui::TestAppContext,
) {
    limit_switch(cx, false, false);
}

#[gpui::test]
fn switching_blank_accounts_reuses_the_session_and_existing_threads_stay_pinned(
    cx: &mut gpui::TestAppContext,
) {
    let (window, _, id) = crate::shell::tests::window(cx);
    let account = window
        .update(cx, |shell, _, cx| {
            let center = crate::accounts::get(cx).unwrap();
            let account = center.update(cx, |center, _| {
                center.book.add(Kind::Claude, "Work").unwrap()
            });
            locked(&shell.store)
                .unwrap()
                .save_provider_accounts(&center.read(cx).book)
                .unwrap();
            let count = shell.sessions.len();
            shell.select_provider_account(
                Kind::Claude,
                &account.id,
                "/work/vorkspace",
                Some(id),
                cx,
            );
            assert_eq!(shell.sessions.len(), count);
            assert_eq!(shell.sessions[&id].read(cx).settings.account.id, account.id);
            shell.sessions[&id].update(cx, |v, _| v.summary.has_messages = true);
            shell.select_provider_account(
                Kind::Claude,
                vorkspace_core::accounts::DEFAULT,
                "/work/vorkspace",
                Some(id),
                cx,
            );
            account
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert_eq!(shell.sessions[&id].read(cx).settings.account.id, account.id);
            assert!(shell.sessions.values().any(|v| {
                let v = v.read(cx);
                v.summary.id != id
                    && v.summary.provider == "claude"
                    && v.settings.account.id == vorkspace_core::accounts::DEFAULT
            }));
        })
        .unwrap();
}
