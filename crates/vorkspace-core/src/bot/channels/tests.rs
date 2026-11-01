use super::*;
fn roster() -> Roster {
    Roster {
        bots: vec![
            Bot {
                id: "a".into(),
                name: "Alpha".into(),
                ..Default::default()
            },
            Bot {
                id: "b".into(),
                name: "Beta".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}
#[test]
fn mention_boundaries_and_member_edits_preserve_pending_intent() {
    let mut r = roster();
    r.bots[1].name = "Alpha Team++".into();
    let mut state = Channels::default();
    let id = state
        .create("team", "", vec!["a".into(), "b".into()], &r)
        .unwrap();
    state
        .post(
            &id,
            Some(&r.bots[0]),
            "@Alpha Team++ check user@Alpha.example",
            "one",
            1,
            &[],
            &r,
        )
        .unwrap();
    assert_eq!(
        state
            .get(&id)
            .unwrap()
            .deliveries
            .iter()
            .map(|d| d.bot.as_str())
            .collect::<Vec<_>>(),
        ["b"]
    );
    let before = serde_json::to_string(&state).unwrap();
    assert!(state
        .edit(&id, "renamed", "topic", vec!["a".into()], &r)
        .is_err());
    assert_eq!(serde_json::to_string(&state).unwrap(), before);
    state
        .edit(&id, "renamed", "topic", vec!["b".into(), "a".into()], &r)
        .unwrap();
    assert_eq!(state.get(&id).unwrap().messages.len(), 1);
    assert_eq!(state.get(&id).unwrap().topic, "topic");
    state
        .post(
            &id,
            Some(&r.bots[0]),
            "@Alpha Team++ final",
            "cap",
            MAX_HOPS,
            &[],
            &r,
        )
        .unwrap();
    assert_eq!(state.get(&id).unwrap().deliveries.len(), 1);
}
#[test]
fn sending_and_clearing_the_draft_reopen_together() {
    for shared in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.db");
        let open = || {
            if shared {
                Store::open_vorkspace(&path)
            } else {
                Store::open(&path)
            }
        };
        let store = open().unwrap();
        let r = roster();
        let mut state = Channels::default();
        let id = state.create("team", "", vec!["a".into()], &r).unwrap();
        let draft = format!("channel-draft:{id}");
        state
            .save_with_draft(&store, &draft, "Unsaved message")
            .unwrap();
        state
            .post(&id, None, "Sent message", "send", 0, &[], &r)
            .unwrap();
        state.save_with_draft(&store, &draft, "").unwrap();
        drop(store);
        let store = open().unwrap();
        assert_eq!(store.setting(&draft).unwrap().as_deref(), Some(""));
        assert_eq!(
            Channels::load(&store)
                .unwrap()
                .get(&id)
                .unwrap()
                .messages
                .len(),
            1
        );
    }
}
#[test]
fn post_save_reopen_claim_and_crash_do_not_replay_effects() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vorkspace.db");
    let store = Store::open_vorkspace(&path).unwrap();
    let r = roster();
    let mut state = Channels::default();
    let id = state
        .create("#Review", "Review code", vec!["a".into(), "b".into()], &r)
        .unwrap();
    let msg = state
        .post(&id, None, "@Alpha inspect this", "request", 0, &[], &r)
        .unwrap();
    assert_eq!(
        state
            .post(&id, None, "@Alpha inspect this", "request", 0, &[], &r)
            .unwrap(),
        msg
    );
    assert!(state
        .post(&id, None, "different", "request", 0, &[], &r)
        .is_err());
    assert_eq!(state.get(&id).unwrap().deliveries.len(), 1);
    state.get_mut(&id).unwrap().deliveries[0].state = "running".into();
    state.save(&store).unwrap();
    drop(store);
    let store = Store::open_vorkspace(&path).unwrap();
    let mut restored = Channels::load(&store).unwrap();
    restored.recover();
    assert_eq!(
        restored.get(&id).unwrap().deliveries[0].state,
        "interrupted"
    );
    assert_eq!(restored.get(&id).unwrap().messages.len(), 1);
}
#[test]
fn autonomous_hops_stop_and_nonmembers_cannot_send() {
    let r = roster();
    let mut s = Channels::default();
    let id = s
        .create("Review", "", vec!["a".into(), "b".into()], &r)
        .unwrap();
    s.post(&id, Some(&r.bots[0]), "@Beta reply", "first", 4, &[], &r)
        .unwrap();
    assert!(s.get(&id).unwrap().deliveries.is_empty());
    s.post(&id, Some(&r.bots[0]), "@Beta reply", "second", 3, &[], &r)
        .unwrap();
    assert_eq!(s.get(&id).unwrap().deliveries[0].hop, 4);
    let outsider = Bot::default();
    assert!(s
        .post(&id, Some(&outsider), "hello", "third", 0, &[], &r)
        .is_err());
}
