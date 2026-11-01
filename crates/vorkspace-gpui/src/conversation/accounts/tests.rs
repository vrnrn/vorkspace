use super::*;

#[gpui::test]
fn bot_usage_notice_renders_model_recovery_without_duplicate_hover_styles(
    cx: &mut gpui::TestAppContext,
) {
    let store = Arc::new(std::sync::Mutex::new(
        vorkspace_core::store::Store::open(Path::new(":memory:")).unwrap(),
    ));
    let summary = locked(&store).unwrap().create("codex", "/fixture").unwrap();
    let view = cx.new(|cx| Conversation::new(summary, store, cx));
    view.update(cx, |view, cx| {
        view.settings.usage_limit = Some(Default::default());
        assert!(view.usage_notice(true, cx).is_some());
    });
}

#[gpui::test]
fn account_recovery_uses_the_terminal_limit_without_reviving_an_older_failure(
    cx: &mut gpui::TestAppContext,
) {
    use vorkspace_core::transcript::Row;
    let store = Arc::new(std::sync::Mutex::new(
        vorkspace_core::store::Store::open(Path::new(":memory:")).unwrap(),
    ));
    let summary = locked(&store)
        .unwrap()
        .create("claude", "/fixture")
        .unwrap();
    let view = cx.new(|cx| Conversation::new(summary, store, cx));
    view.update(cx, |view, cx| {
        let row = |role: &str, text: &str| Row {
            role: role.into(),
            text: text.into(),
            first: true,
            sent_at: None,
        };
        for role in ["error", "system"] {
            view.transcript.update(cx, |v, cx| {
                v.patch(
                    0,
                    vec![row("user", "Keep going"), row(role, "rate_limit")],
                    cx,
                );
            });
            assert!(view.account_recovery_limit(cx).is_some());
        }
        view.transcript.update(cx, |v, cx| {
            v.patch(
                2,
                vec![row("user", "Try again"), row("assistant", "Completed")],
                cx,
            );
        });
        assert!(view.account_recovery_limit(cx).is_none());
        view.transcript.update(cx, |v, cx| {
            v.patch(4, vec![row("system", "Network timeout")], cx);
        });
        assert!(view.account_recovery_limit(cx).is_none());
    });
}

#[gpui::test]
fn claude_limit_code_opens_recovery_and_preserves_the_reported_reset(
    cx: &mut gpui::TestAppContext,
) {
    let store = Arc::new(std::sync::Mutex::new(
        vorkspace_core::store::Store::open(Path::new(":memory:")).unwrap(),
    ));
    let summary = locked(&store)
        .unwrap()
        .create("claude", "/fixture")
        .unwrap();
    let view = cx.new(|cx| Conversation::new(summary, store, cx));
    view.update(cx, |view, cx| {
        view.status = "rate_limit".into();
        view.detect_usage_limit(cx);
        assert!(view.settings.usage_limit.is_some());
        assert!(view.follow_ups.state.paused);
        let reported = Limit {
            resets_at: Some(usage::now_ms() + 3_600_000),
            resume_at_reset: false,
        };
        view.settings.usage_limit = Some(reported.clone());
        view.detect_usage_limit(cx);
        assert_eq!(view.settings.usage_limit, Some(reported));
    });
}

#[gpui::test]
fn usage_banner_matches_the_composer_and_can_arm_cancel_and_dismiss(cx: &mut gpui::TestAppContext) {
    let store = Arc::new(std::sync::Mutex::new(
        vorkspace_core::store::Store::open(Path::new(":memory:")).unwrap(),
    ));
    let mut summary = locked(&store)
        .unwrap()
        .create("claude", "/fixture")
        .unwrap();
    summary.has_messages = true;
    let id = summary.id;
    let saved = store.clone();
    let window = cx.update(|cx| {
        crate::accounts::install(store.clone(), cx);
        crate::accounts::get(cx).unwrap().update(cx, |center, _| {
            center.book.add(Kind::Claude, "Other account").unwrap();
        });
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut view = Conversation::new(summary, store, cx);
                view.loaded = true;
                view.status = "Claude Code reached its usage limit.".into();
                view.settings.usage_limit = Some(Limit {
                    resets_at: Some(usage::now_ms() + 3_600_000),
                    resume_at_reset: false,
                });
                view
            })
        })
        .unwrap()
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    let banner = visual.debug_bounds("usage-limit-banner").unwrap();
    let composer = visual.debug_bounds("composer-box").unwrap();
    let resume = visual.debug_bounds("resume-usage-limit").unwrap();
    let account = visual.debug_bounds("usage-account-picker").unwrap();
    let dismiss = visual.debug_bounds("dismiss-usage-limit").unwrap();
    assert_eq!(banner.size.height, px(32.));
    assert_eq!(resume.size.height, px(24.));
    assert_eq!(dismiss.size, gpui::size(px(24.), px(24.)));
    assert_eq!(banner.bottom(), composer.top());
    assert_eq!(banner.left(), composer.left() + px(8.));
    assert_eq!(banner.right(), composer.right() - px(8.));
    assert_eq!(account.right() + px(8.), resume.left());
    assert_eq!(resume.right() + px(8.), dismiss.left());
    assert!(visual.debug_bounds("usage-model-picker").is_none());
    assert!(visual.debug_bounds("provider-status").is_none());

    visual.simulate_click(resume.center(), Default::default());
    assert!(
        window
            .update(visual, |v, _, _| v
                .settings
                .usage_limit
                .as_ref()
                .unwrap()
                .resume_at_reset)
            .unwrap()
    );
    assert!(
        locked(&saved)
            .unwrap()
            .agent_settings(id)
            .unwrap()
            .unwrap()
            .usage_limit
            .unwrap()
            .resume_at_reset
    );
    visual.refresh().unwrap();
    let resume = visual.debug_bounds("resume-usage-limit").unwrap();
    visual.simulate_click(resume.center(), Default::default());
    assert!(
        !window
            .update(visual, |v, _, _| v
                .settings
                .usage_limit
                .as_ref()
                .unwrap()
                .resume_at_reset)
            .unwrap()
    );

    // Unknown or elapsed reset times keep a manual Resume control and the
    // same right-aligned actions, as in React's composer variant.
    for resets_at in [None, Some(usage::now_ms() - 1_000)] {
        window
            .update(visual, |v, _, cx| {
                v.settings.usage_limit.as_mut().unwrap().resets_at = resets_at;
                cx.notify();
            })
            .unwrap();
        visual.refresh().unwrap();
        assert_eq!(
            visual.debug_bounds("resume-usage-limit").unwrap().right(),
            resume.right()
        );
    }
    visual.simulate_click(dismiss.center(), Default::default());
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("usage-limit-notice").is_none());
    assert!(
        window
            .update(visual, |v, _, _| v.settings.usage_limit.is_none())
            .unwrap()
    );
}

#[gpui::test]
fn failed_account_transfer_preserves_the_original_account_and_resume_binding(
    cx: &mut gpui::TestAppContext,
) {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(std::sync::Mutex::new(
        vorkspace_core::store::Store::open(Path::new(":memory:")).unwrap(),
    ));
    let mut summary = locked(&store).unwrap().create("codex", "/fixture").unwrap();
    locked(&store)
        .unwrap()
        .provider_id(summary.id, "missing-source-thread")
        .unwrap();
    summary.provider_id = Some("missing-source-thread".into());
    let id = summary.id;
    let source = Profile {
        id: "source".into(),
        root: Some(directory.path().into()),
    };
    let target = Profile {
        id: "target".into(),
        root: Some(directory.path().into()),
    };
    let view = cx.new(|cx| Conversation::new(summary, store.clone(), cx));
    cx.run_until_parked();
    view.update(cx, |view, cx| {
        view.settings.account = source.clone();
        view.status = "Usage limit reached: fixture".into();
        view.switch_usage_account(target, cx);
        assert!(view.account_switching);
    });
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert!(!view.account_switching);
        assert_eq!(view.settings.account, source);
        assert_eq!(
            view.summary.provider_id.as_deref(),
            Some("missing-source-thread")
        );
        assert!(view.settings.pending_provider_switch.is_none());
        assert!(view.settings.usage_limit.is_some());
        assert!(view.status.starts_with("Could not switch accounts:"));
        assert!(!view.busy);
    });
    let summaries = locked(&store).unwrap().sessions().unwrap();
    assert_eq!(
        summaries
            .iter()
            .find(|s| s.id == id)
            .unwrap()
            .provider_id
            .as_deref(),
        Some("missing-source-thread")
    );
}

#[gpui::test]
fn subscription_limits_persist_and_release_a_bot_only_once_when_armed(
    cx: &mut gpui::TestAppContext,
) {
    let store = Arc::new(std::sync::Mutex::new(
        vorkspace_core::store::Store::open(Path::new(":memory:")).unwrap(),
    ));
    let summary = locked(&store).unwrap().create("codex", "/fixture").unwrap();
    let id = summary.id;
    let view = cx.new(|cx| Conversation::new(summary, store.clone(), cx));
    cx.run_until_parked();
    view.update(cx, |v, cx| {
        v.settings.bot = Some(vorkspace_core::agent::BotContext {
            bot: Default::default(),
            root: "/fixture".into(),
            executable: "/fixture/binary".into(),
        });
        v.status = "Usage limit reached: Fixture subscription exhausted".into();
        v.detect_usage_limit(cx);
        assert!(v.settings.usage_limit.is_some());
        assert!(v.follow_ups.state.paused);
    });
    cx.run_until_parked();
    assert!(
        locked(&store)
            .unwrap()
            .agent_settings(id)
            .unwrap()
            .unwrap()
            .usage_limit
            .is_some()
    );
    let resumed = std::rc::Rc::new(std::cell::Cell::new(0));
    cx.update(|cx| {
        let count = resumed.clone();
        cx.subscribe(&view, move |_, _: &UsageResumed, _| {
            count.set(count.get() + 1)
        })
        .detach();
    });
    view.update(cx, |v, cx| {
        v.settings.usage_limit.as_mut().unwrap().resets_at = Some(usage::now_ms() - 31_000);
        v.tick_usage(cx);
        assert!(v.settings.usage_limit.is_some());
        v.settings.usage_limit.as_mut().unwrap().resume_at_reset = true;
        v.tick_usage(cx);
        assert!(v.settings.usage_limit.is_none());
        v.tick_usage(cx);
    });
    cx.run_until_parked();
    assert_eq!(resumed.get(), 1);
}
