use super::*;
use vorkspace_core::{agent::BotContext, bot::Bot, store::Store};

fn fixture(cx: &mut Context<Conversation>) -> Conversation {
    let store = Store::test(Path::new(":memory:")).unwrap();
    let summary = store
        .create("codex", "/tmp/native-recovery-fixture")
        .unwrap();
    let mut view = Conversation::new(summary, Arc::new(std::sync::Mutex::new(store)), cx);
    view.loaded = true;
    view.summary.provider_id = Some("fixture-thread".into());
    view.settings.bot = Some(BotContext {
        bot: Bot::default(),
        root: "/tmp/native-recovery-fixture".into(),
        executable: Default::default(),
    });
    view.native_recovery = Some(Recovery {
        token: "fixture-token".into(),
        session: view.summary.id,
        provider: view.summary.provider.clone(),
        native_id: view.summary.provider_id.clone(),
        account: view.settings.account.id.clone(),
    });
    view
}

#[gpui::test]
fn a_recovery_confirmation_cannot_follow_a_replacement_native_session(
    cx: &mut gpui::TestAppContext,
) {
    let window = cx.update(|cx| {
        input::bind(cx);
        cx.open_window(Default::default(), |_, cx| cx.new(fixture))
            .unwrap()
    });
    window
        .update(cx, |view, window, cx| {
            view.confirm_native_recovery(window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            view.summary.provider_id = Some("replacement-thread".into());
            view.status = "Replacement ready".into();
            assert!(view.native_history_banner(cx).is_none());
        })
        .unwrap();
    cx.simulate_prompt_answer("Resume here");
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| {
            assert!(!view.native_syncing);
            assert_eq!(view.status, "Replacement ready");
        })
        .unwrap();
}

#[test]
#[ignore = "Requires Metal and VORKSPACE_NATIVE_RECOVERY_CAPTURE_DIR"]
fn native_recovery_banner_capture() {
    struct Capture(Entity<Conversation>);
    impl gpui::Render for Capture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let banner = self.0.update(cx, |view, cx| view.native_history_banner(cx));
            div()
                .size_full()
                .p_4()
                .bg(vorkspace_ui::theme::pane_fill())
                .children(banner)
        }
    }
    let output = std::path::PathBuf::from(
        std::env::var_os("VORKSPACE_NATIVE_RECOVERY_CAPTURE_DIR").unwrap(),
    );
    assert!(output.is_absolute() && output.is_dir());
    let mut cx = gpui::HeadlessAppContext::with_platform(
        Arc::new(gpui_apple::AppleTextSystem::new()),
        Arc::new(crate::assets::Assets),
        || {
            Ok(Some(Box::new(
                gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
            )))
        },
    );
    for light in [false, true] {
        let prefs = vorkspace_ui::preferences::Preferences {
            theme: if light { "light" } else { "dark" }.into(),
            ..Default::default()
        };
        vorkspace_ui::theme::configure(&prefs, false, true);
        cx.update(|cx| {
            input::bind(cx);
            cx.set_global(prefs);
        });
        for width in [360., 740.] {
            let window = cx
                .open_window(gpui::size(px(width), px(84.)), |_, cx| {
                    let view = cx.new(fixture);
                    cx.new(|_| Capture(view))
                })
                .unwrap();
            cx.run_until_parked();
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(output.join(format!(
                    "recovery-{}-{width}.png",
                    if light { "light" } else { "dark" }
                )))
                .unwrap();
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
        }
    }
    vorkspace_ui::theme::configure(&Default::default(), false, true);
}
