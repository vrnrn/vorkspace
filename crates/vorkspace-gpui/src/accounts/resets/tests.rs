
use super::*;
use std::sync::atomic::AtomicUsize;

fn snapshot(count: u64) -> Snapshot {
    Snapshot {
        reset_credits: Some(usage::ResetCredits {
            available_count: count,
            credits: Vec::new(),
        }),
        ..Default::default()
    }
}

#[test]
fn banked_rows_keep_details_filter_spent_credits_and_fill_aggregate_count() {
    let mut data = snapshot(3);
    data.reset_credits.as_mut().unwrap().credits =
        ["available", "redeemed", "unknown", "redeeming"]
            .into_iter()
            .enumerate()
            .map(|(i, status)| usage::ResetCredit {
                id: i.to_string(),
                status: status.into(),
                reset_type: "codexRateLimits".into(),
                granted_at: None,
                expires_at: Some(2_000_000),
                title: Some("A reward".into()),
                description: Some("One Codex reset".into()),
            })
            .collect();
    let cards = rows(Some(&data));
    assert_eq!(cards.len(), 3);
    assert_eq!(cards[0].key, "credit:0");
    assert_eq!(cards[1].key, "credit:2");
    assert_eq!(cards[0].title, "A reward");
    assert_eq!(
        cards[0].credit.as_ref().unwrap().expires_at,
        Some(2_000_000)
    );
    assert_eq!(
        cards[0].credit.as_ref().unwrap().description.as_deref(),
        Some("One Codex reset")
    );
    assert_eq!(cards[2].key, "unlisted:2");
    assert!(cards[2].credit.is_none());
    assert_eq!(rows(Some(&snapshot(2))).len(), 2);
    assert!(rows(Some(&snapshot(0))).is_empty());
    assert!(rows(None).is_empty());
}

#[gpui::test]
fn banked_claims_are_serialized_per_account_and_refresh_the_right_cache(
    cx: &mut gpui::TestAppContext,
) {
    let store = Arc::new(std::sync::Mutex::new(
        vorkspace_core::store::Store::open(Path::new(":memory:")).unwrap(),
    ));
    let center = cx.new(|cx| Accounts::new(PathBuf::from("/fixture"), Book::default(), store, cx));
    let calls = Arc::new(AtomicUsize::new(0));
    let key = Key::new(Kind::Codex, Profile::default(), None);
    center.update(cx, |accounts, cx| {
        accounts.book.accounts.push(Account {
            id: "other".into(),
            provider: "codex".into(),
            label: "Other".into(),
        });
        accounts
            .usage
            .entry(key.clone())
            .or_default()
            .complete(Ok(snapshot(2)));
        let other = Key::new(Kind::Codex, accounts.profile("other"), None);
        let mut other_snapshot = snapshot(3);
        other_snapshot
            .reset_credits
            .as_mut()
            .unwrap()
            .credits
            .push(usage::ResetCredit {
                id: "specific-credit".into(),
                status: "available".into(),
                reset_type: "codexRateLimits".into(),
                granted_at: None,
                expires_at: None,
                title: None,
                description: None,
            });
        accounts
            .usage
            .entry(other.clone())
            .or_default()
            .complete(Ok(other_snapshot));
        for _ in 0..2 {
            let calls = calls.clone();
            accounts.consume_reset_with(
                key.clone(),
                "unlisted:0",
                move |profile, credit| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(profile.id, accounts::DEFAULT);
                    assert!(credit.is_none());
                    (Ok("reset".into()), Ok(snapshot(1)))
                },
                cx,
            );
        }
        accounts.consume_reset_with(
            other.clone(),
            "credit:specific-credit",
            |profile, credit| {
                assert_eq!(profile.id, "other");
                assert_eq!(credit, Some("specific-credit"));
                (Ok("nothingToReset".into()), Ok(snapshot(3)))
            },
            cx,
        );
        assert!(accounts.reset_actions[&key].applying());
        assert!(accounts.reset_actions[&other].applying());
    });
    cx.run_until_parked();
    center.update(cx, |accounts, _| {
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            accounts.usage[&key]
                .snapshot
                .as_ref()
                .unwrap()
                .reset_credits
                .as_ref()
                .unwrap()
                .available_count,
            1
        );
        assert_eq!(
            accounts.reset_actions[&key].message(),
            Some("Codex usage was reset.")
        );
        assert!(accounts.reset_actions[&key].row.is_none());
        let other = Key::new(Kind::Codex, accounts.profile("other"), None);
        assert_eq!(
            accounts.reset_actions[&other].message(),
            Some("There’s no active usage to reset.")
        );
        assert!(!accounts.usage[&key].fetching);
    });
}

#[gpui::test]
fn banked_claim_errors_preserve_credits_and_invalidated_replies_stay_discarded(
    cx: &mut gpui::TestAppContext,
) {
    let store = Arc::new(std::sync::Mutex::new(
        vorkspace_core::store::Store::open(Path::new(":memory:")).unwrap(),
    ));
    let center = cx.new(|cx| Accounts::new(PathBuf::from("/fixture"), Book::default(), store, cx));
    let key = Key::new(Kind::Codex, Profile::default(), None);
    center.update(cx, |accounts, cx| {
        accounts
            .usage
            .entry(key.clone())
            .or_default()
            .complete(Ok(snapshot(2)));
        accounts.consume_reset_with(
            key.clone(),
            "unlisted:0",
            |_, _| {
                (
                    Err("Network unavailable".into()),
                    Err(FetchError {
                        message: "Refresh failed".into(),
                        unavailable: false,
                    }),
                )
            },
            cx,
        );
    });
    cx.run_until_parked();
    center.update(cx, |accounts, cx| {
        assert_eq!(rows(accounts.usage[&key].snapshot.as_ref()).len(), 2);
        assert_eq!(
            accounts.reset_actions[&key].message(),
            Some("Network unavailable")
        );
        assert!(!accounts.reset_actions[&key].applying());
        accounts.consume_reset_with(
            key.clone(),
            "unlisted:0",
            |_, _| (Ok("reset".into()), Ok(snapshot(0))),
            cx,
        );
        // Match sign-out's generation change while the old account request is pending.
        let cached = accounts.usage.get_mut(&key).unwrap();
        cached.generation += 1;
        cached.complete(Ok(snapshot(4)));
        accounts.reset_actions.remove(&key);
    });
    cx.run_until_parked();
    center.update(cx, |accounts, _| {
        assert_eq!(rows(accounts.usage[&key].snapshot.as_ref()).len(), 4);
        assert!(!accounts.reset_actions.contains_key(&key));
    });
}
