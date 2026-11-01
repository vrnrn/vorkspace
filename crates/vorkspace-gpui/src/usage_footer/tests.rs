#[test]
fn sidebar_usage_windows_resets_and_rail_priority() {
    use super::*;
    let window = |minutes, used_percent, resets_at| usage::Window {
        minutes,
        used_percent,
        resets_at,
    };
    let mut snapshot = usage::Snapshot {
        windows: vec![
            window(10_080, 31., Some(172_800_000)),
            window(300, 0., None),
        ],
        ..Default::default()
    };
    assert_eq!(
        summary_lines(Some(&snapshot), false, 0),
        ["5h: 100% left", "W: 69% left · 2d"]
    );
    assert_eq!(rail_window(Some(&snapshot)).unwrap().minutes, 10_080);
    snapshot.windows[1].resets_at = Some(3_600_000);
    assert_eq!(
        summary_lines(Some(&snapshot), true, 0),
        ["5h: 100% left · 1h", "W: 69% left · 2d"]
    );
    snapshot.windows.remove(1);
    assert_eq!(summary_lines(Some(&snapshot), true, 0), ["69% left · 2d"]);
    assert_eq!(rail_window(Some(&snapshot)).unwrap().minutes, 10_080);
    snapshot.windows = vec![window(300, 0., Some(3_600_000))];
    assert_eq!(summary_lines(Some(&snapshot), false, 0), ["0% used · 1h"]);
    assert_eq!(rail_window(Some(&snapshot)).unwrap().minutes, 300);
    snapshot.windows = vec![window(1_440, 75., None)];
    assert_eq!(summary_lines(Some(&snapshot), true, 0), ["25% left"]);
    assert_eq!(rail_window(Some(&snapshot)).unwrap().minutes, 1_440);
    snapshot.windows[0].used_percent = f64::NAN;
    assert!(summary_lines(Some(&snapshot), true, 0).is_empty());
    assert!(rail_window(Some(&snapshot)).is_none());
    assert!(rail_window(None).is_none());
    snapshot.windows.clear();
    assert!(rail_window(Some(&snapshot)).is_none());
}

use super::*;
#[gpui::test]
fn subscription_pointer_and_keyboard_activation_refresh_the_selected_account(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(bind);
    let directory = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(std::sync::Mutex::new(
        vorkspace_core::store::Store::open(&directory.path().join("native.sqlite")).unwrap(),
    ));
    let keys = cx.update(|cx| {
        crate::accounts::install(store, cx);
        crate::accounts::get(cx).unwrap().update(cx, |center, cx| {
            let keys = [Kind::Codex, Kind::Claude]
                .map(|kind| Key::new(kind, center.profile("default"), None));
            for key in &keys {
                center
                    .usage
                    .entry(key.clone())
                    .or_default()
                    .complete(Ok(Snapshot {
                        windows: vec![usage::Window {
                            used_percent: 10.,
                            minutes: 300,
                            resets_at: None,
                        }],
                        updated_at: usage::now_ms(),
                        ..Default::default()
                    }));
            }
            cx.notify();
            keys
        })
    });
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| UsageFooter::new(Kind::Codex, cx))
        })
        .unwrap()
    });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let id = |key: &Key| -> &'static str {
        format!(
            "subscription-{}-{}-{}",
            key.kind.id(),
            key.profile.id,
            key.billing
        )
        .leak()
    };
    let a = visual.debug_bounds(id(&keys[0])).unwrap();
    let b = visual.debug_bounds(id(&keys[1])).unwrap();
    assert!(b.origin.y > a.origin.y, "rows stack");
    assert!(a.size.width > px(100.), "rows are detailed, not rings");
    assert!(visual.debug_bounds("subscription-refresh").is_none());
    visual.simulate_click(b.center(), gpui::Modifiers::none());
    visual.run_until_parked();
    let requested = window
        .read_with(&visual, |footer, _| footer.refresh_requests.clone())
        .unwrap();
    assert_eq!(requested, vec![keys[1].clone()]);

    for compact in [false, true] {
        for (keystroke, key) in [("enter", &keys[0]), ("space", &keys[1])] {
            window
                .update(&mut visual, |footer, window, cx| {
                    footer.dismiss(window, cx);
                    footer.set_compact(compact, cx);
                    footer.refresh_requests.clear();
                    window.focus(&footer.rows[key].1, cx);
                })
                .unwrap();
            visual.run_until_parked();
            visual.simulate_keystrokes(keystroke);
            visual.run_until_parked();
            window
                .update(&mut visual, |footer, _, _| {
                    assert_eq!(footer.refresh_requests, vec![key.clone()]);
                    assert_eq!(footer.open_subscription_key(), Some(key));
                })
                .unwrap();
            visual.simulate_keystrokes("escape");
            visual.run_until_parked();
            window
                .update(&mut visual, |footer, window, _| {
                    assert!(!footer.open);
                    assert!(footer.rows[key].1.is_focused(window));
                    assert_eq!(footer.refresh_requests, vec![key.clone()]);
                })
                .unwrap();
        }
    }
}

#[gpui::test]
fn subscriptions_keep_account_usage_separate_from_the_active_conversation(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(bind);
    let directory = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(std::sync::Mutex::new(
        vorkspace_core::store::Store::open(&directory.path().join("native.sqlite")).unwrap(),
    ));
    let work = cx.update(|cx| {
        crate::accounts::install(store, cx);
        crate::accounts::get(cx).unwrap().update(cx, |center, cx| {
            let work = center.book.add(Kind::Codex, "Work").unwrap();
            for (kind, id, percent) in [
                (Kind::Codex, "default", 20.),
                (Kind::Claude, "default", 40.),
                (Kind::Codex, work.id.as_str(), 90.),
            ] {
                let key = Key::new(kind, center.profile(id), None);
                center.usage.entry(key).or_default().complete(Ok(Snapshot {
                    windows: vec![usage::Window {
                        used_percent: percent,
                        minutes: 300,
                        resets_at: None,
                    }],
                    updated_at: usage::now_ms(),
                    ..Default::default()
                }));
            }
            cx.notify();
            Key::new(Kind::Codex, center.profile(&work.id), None)
        })
    });
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| UsageFooter::new(Kind::Codex, cx))
        })
        .unwrap()
    });
    cx.run_until_parked();
    window
        .update(cx, |usage, window, cx| {
            let rows = usage.subscriptions(cx);
            assert_eq!(rows.len(), 3);
            assert!(
                rows.iter()
                    .any(|row| row.0 == work && row.1 == "Codex · Work")
            );
            usage.open_subscription(work.clone(), window, cx);
            assert_eq!(
                usage.cache[index(Kind::Codex)]
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .windows[0]
                    .used_percent,
                90.
            );
            usage.context(
                Kind::Codex,
                Default::default(),
                None,
                "/work".into(),
                Some(7),
                cx,
            );
            assert!(
                usage.open,
                "unchanged chat context must not close a different account's details"
            );
            assert_eq!(usage.key, work);
        })
        .unwrap();
    cx.update(|cx| {
        crate::accounts::get(cx).unwrap().update(cx, |center, cx| {
            let key = Key::new(Kind::Codex, Default::default(), None);
            center
                .usage
                .get_mut(&key)
                .unwrap()
                .snapshot
                .as_mut()
                .unwrap()
                .windows[0]
                .used_percent = 5.;
            cx.notify();
        });
    });
    cx.run_until_parked();
    window
        .update(cx, |usage, _, cx| {
            assert_eq!(
                usage.cache[index(Kind::Codex)]
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .windows[0]
                    .used_percent,
                90.
            );
            usage.set_remaining(true, cx);
            assert_eq!(usage.percent_label(90.), "10% left");
            usage.hide(cx);
            assert!(!usage.open);
            crate::accounts::get(cx).unwrap().update(cx, |center, cx| {
                center
                    .book
                    .accounts
                    .retain(|account| account.id != work.profile.id);
                cx.notify();
            });
            assert_eq!(
                usage.subscriptions(cx).len(),
                2,
                "removed accounts cannot survive as stale usage rows"
            );
            let bedrock = Key::new(Kind::Claude, Default::default(), None);
            crate::accounts::get(cx).unwrap().update(cx, |center, cx| {
                center
                    .usage
                    .get_mut(&bedrock)
                    .unwrap()
                    .complete(Ok(Snapshot {
                        external_billing: Some("AWS Bedrock".into()),
                        updated_at: usage::now_ms(),
                        ..Default::default()
                    }));
                cx.notify();
            });
            assert!(
                usage.subscriptions(cx).iter().any(|row| row.0 == bedrock),
                "Bedrock must remain visible without subscription limit windows"
            );
            usage.select_subscription(bedrock, cx);
            let snapshot = usage.cache[index(Kind::Claude)].snapshot.as_ref();
            assert_eq!(external_billing(snapshot), Some("AWS Bedrock"));
            assert!(
                !usage.login_view(cx),
                "Bedrock must not prompt for Claude OAuth"
            );
            assert!(snapshot.unwrap().windows.is_empty());
        })
        .unwrap();
    // Without a cost monitoring link the row keeps opening its details.
    window
        .update(cx, |usage, w, cx| {
            let bedrock = Key::new(Kind::Claude, Default::default(), None);
            usage.open_subscription(bedrock, w, cx);
            assert!(usage.open);
            usage.hide(cx);
        })
        .unwrap();
    assert_eq!(cx.opened_url(), None);
    cx.update(|cx| {
        cx.set_global(vorkspace_ui::preferences::Preferences {
            bedrock_cost_url: " https://console.aws.amazon.com/costmanagement/ ".into(),
            ..Default::default()
        })
    });
    window
        .update(cx, |usage, w, cx| {
            let bedrock = Key::new(Kind::Claude, Default::default(), None);
            usage.open_subscription(bedrock, w, cx);
            assert!(!usage.open, "the link replaces the details popover");
        })
        .unwrap();
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://console.aws.amazon.com/costmanagement/")
    );
}
#[test]
fn cost_link_accepts_only_plain_web_links() {
    let link = |url: &str| {
        vorkspace_ui::preferences::Preferences {
            bedrock_cost_url: url.into(),
            ..Default::default()
        }
        .cost_link()
        .map(str::to_owned)
    };
    assert_eq!(
        link(" http://a.example/x ").as_deref(),
        Some("http://a.example/x")
    );
    for bad in [
        "",
        "console.aws.amazon.com",
        "javascript:alert(1)",
        "file:///etc/passwd",
        "https://",
        "https:///x",
        "https://a b.example",
        "ftp://a.example",
    ] {
        assert_eq!(link(bad), None, "{bad}");
    }
}
#[gpui::test]
fn details_open_with_keyboard_escape_restores_focus_and_provider_caches_stay_separate(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(bind);
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| UsageFooter::new(Kind::Codex, cx))
        })
        .unwrap()
    });
    window
        .update(cx, |footer, w, cx| {
            footer.cache[0].complete(Ok(Snapshot {
                windows: vec![usage::Window {
                    used_percent: 28.,
                    minutes: 300,
                    resets_at: None,
                }],
                updated_at: usage::now_ms(),
                ..Default::default()
            }));
            w.focus(&footer.trigger, cx);
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.simulate_keystrokes(window.into(), "enter");
    cx.run_until_parked();
    window
        .update(cx, |footer, w, cx| {
            assert!(footer.open);
            assert!(footer.details.is_focused(w));
            cx.notify();
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "escape");
    cx.run_until_parked();
    window
        .update(cx, |footer, w, cx| {
            assert!(!footer.open);
            assert!(footer.trigger.is_focused(w));
            footer.provider(Kind::Claude, cx);
            assert!(footer.cache[index(footer.kind)].snapshot.is_none());
            // A late Codex response belongs to Codex, while Claude stays in view.
            footer.cache[0].complete(Ok(Snapshot {
                windows: vec![usage::Window {
                    used_percent: 42.,
                    minutes: 300,
                    resets_at: None,
                }],
                updated_at: usage::now_ms(),
                ..Default::default()
            }));
            assert!(footer.cache[index(footer.kind)].snapshot.is_none());
            footer.provider(Kind::Codex, cx);
            assert_eq!(
                footer.cache[0].snapshot.as_ref().unwrap().windows[0].used_percent,
                42.
            );
        })
        .unwrap();
}
#[test]
fn failed_refresh_keeps_snapshot_but_sign_out_clears_it() {
    let mut state = CachedUsage {
        snapshot: Some(Snapshot::default()),
        fetching: true,
        ..Default::default()
    };
    state.complete(Err(FetchError {
        message: "Offline".into(),
        unavailable: false,
    }));
    assert!(state.snapshot.is_some());
    assert!(!state.fetching);
    state.complete(Err(FetchError {
        message: "Signed out".into(),
        unavailable: true,
    }));
    assert!(state.snapshot.is_none());
    state.complete(Ok(Snapshot::default()));
    assert!(state.error.is_none());
}
