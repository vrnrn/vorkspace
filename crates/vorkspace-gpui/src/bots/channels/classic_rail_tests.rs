use super::*;
use crate::shell::Shell;
use gpui::{TestAppContext, VisualTestContext};
use std::sync::{Arc, Mutex};
use vorkspace_core::{bot::channels::Channel, store::Store};

#[gpui::test]
fn rail_opens_channels_without_a_selected_bot_and_returns_to_the_workspace(
    cx: &mut TestAppContext,
) {
    let root = std::env::temp_dir().join(format!("classic-channel-{}", bot::new_id()));
    std::fs::create_dir_all(&root).unwrap();
    let store = Arc::new(Mutex::new(
        Store::open(&root.join("native.sqlite")).unwrap(),
    ));
    let project = root.to_string_lossy().into_owned();
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| Shell::new(store, Vec::new(), project, None, cx))
        })
        .unwrap()
    });
    cx.run_until_parked();
    let manager = window
        .update(cx, |shell, _, _| shell.bot_manager())
        .unwrap();
    manager.update(cx, |manager, cx| {
        assert!(manager.active.is_none());
        manager.prefs.channels = true;
        manager.roster.bots.push(Bot {
            id: "reader".into(),
            name: "Reader".into(),
            ..Default::default()
        });
        manager.channels.state.channels.push(Channel {
            id: "team".into(),
            name: "team".into(),
            member_ids: vec!["reader".into()],
            messages: vec![
                json!({"id":"first","from":{"kind":"bot","botId":"reader"},"text":"First message"}),
            ],
            ..Default::default()
        });
        manager.changed(cx);
    });
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1400.), px(900.)));
    window
        .update(visual, |_, window, _| window.activate_window())
        .unwrap();
    visual.run_until_parked();
    let channel = visual.debug_bounds("classic-channel-team").unwrap();
    let label = visual.debug_bounds("channel-label-team").unwrap();
    let bot = visual.debug_bounds("bot-rail-reader").unwrap();
    let project = visual.debug_bounds("classic-project-label-0").unwrap();
    assert_eq!(
        label.left(),
        bot.left() + px(32.),
        "after the 16px mark slot"
    );
    assert_eq!(label.left(), project.left());
    assert!(visual.debug_bounds("channel-unread-team").is_some());
    visual.simulate_click(channel.center(), Default::default());
    visual.run_until_parked();
    manager.read_with(visual, |manager, _| {
        assert!(manager.classic_channels_open());
        assert!(manager.active.is_none());
        assert_eq!(manager.channels.selected.as_deref(), Some("team"));
        assert!(!manager.channel_unread("team"));
    });
    assert!(visual.debug_bounds("channel-unread-team").is_none());
    manager.update(visual, |manager, cx| {
        let mut next = manager.channels.state.clone();
        next.get_mut("team").unwrap().messages.push(
            json!({"id":"visible","from":{"kind":"bot","botId":"reader"},"text":"Visible reply"}),
        );
        manager.commit_channels(next, cx).unwrap();
        assert!(!manager.channel_unread("team"));
    });
    visual.deactivate_window();
    manager.update(visual, |manager, cx| {
            let mut next = manager.channels.state.clone();
            next.get_mut("team").unwrap().messages.push(json!({"id":"background","from":{"kind":"bot","botId":"reader"},"text":"Reply while inactive"}));
            manager.commit_channels(next, cx).unwrap();
            assert!(manager.channel_unread("team"));
            let db = locked(&manager.store).unwrap();
            let seen: HashMap<String, String> = serde_json::from_str(&db.setting(READ_KEY).unwrap().unwrap()).unwrap();
            let restored = Channels::load(&db).unwrap();
            assert_eq!(seen["team"], "visible");
            assert_eq!(restored.get("team").unwrap().messages.last().unwrap()["id"], "background");
        });
    visual.run_until_parked();
    manager.read_with(visual, |manager, cx| {
        assert!(cx.active_window().is_none());
        assert!(manager.channel_unread("team"));
    });
    assert!(visual.debug_bounds("channel-unread-team").is_some());
    window
        .update(visual, |_, window, _| window.activate_window())
        .unwrap();
    visual.refresh().unwrap();
    visual.run_until_parked();
    assert!(visual.debug_bounds("channel-unread-team").is_none());
    assert!(visual.debug_bounds("channel-messages").is_some());
    assert!(visual.debug_bounds("classic-project-rail").is_some());
    assert!(visual.debug_bounds("sidebar-frame").is_none());

    assert!(visual.debug_bounds("close-channels").is_none());
    let close = visual.debug_bounds("classic-project-0").unwrap();
    visual.simulate_click(close.center(), Default::default());
    visual.run_until_parked();
    assert!(!manager.read_with(visual, |manager, _| manager.classic_channels_open()));
    assert!(visual.debug_bounds("sidebar-frame").is_some());

    let add = visual.debug_bounds("classic-add-channel").unwrap();
    visual.simulate_click(add.center(), Default::default());
    visual.run_until_parked();
    manager.read_with(visual, |manager, _| {
        assert!(manager.classic_channels_open());
        assert!(manager.channels.selected.is_none());
    });
    visual.simulate_input("focused-channel-name");
    visual.run_until_parked();
    manager.read_with(visual, |manager, cx| {
        assert_eq!(
            manager.fields["channel-new:name"].read(cx).content,
            "focused-channel-name"
        );
    });
    let project = visual.debug_bounds("classic-project-0").unwrap();
    visual.simulate_click(project.center(), Default::default());
    visual.run_until_parked();
    assert!(!manager.read_with(visual, |manager, _| manager.classic_channels_open()));
    assert!(visual.debug_bounds("sidebar-frame").is_some());

    // Right-clicking a channel in the sidebar opens its appearance options menu.
    let channel = visual.debug_bounds("classic-channel-team").unwrap();
    let at = channel.center();
    visual.simulate_event(gpui::MouseDownEvent {
        position: at,
        button: MouseButton::Right,
        modifiers: Default::default(),
        click_count: 1,
        first_mouse: false,
    });
    visual.simulate_event(gpui::MouseUpEvent {
        position: at,
        button: MouseButton::Right,
        modifiers: Default::default(),
        click_count: 1,
    });
    visual.run_until_parked();
    let grid_slider = visual
        .debug_bounds("channel-grid-slider")
        .expect("right-click opens the channel appearance menu");
    let click_x = grid_slider.left() + grid_slider.size.width * (4. / 6.);
    visual.simulate_click(
        gpui::point(click_x, grid_slider.center().y),
        Default::default(),
    );
    visual.run_until_parked();
    let color_btn = visual.debug_bounds("channel-color-4").unwrap();
    visual.simulate_click(color_btn.center(), Default::default());
    visual.run_until_parked();
    let tex_toggle = visual.debug_bounds("channel-texture-toggle").unwrap();
    visual.simulate_click(tex_toggle.center(), Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("channel-grid-slider").is_none());
    manager.read_with(visual, |manager, _| {
        let ch = manager.channels.state.get("team").unwrap();
        assert_eq!(ch.grid, Some(6));
        assert_eq!(ch.color, Some(COLORS[4]));
        assert!(!ch.textured);
    });
}
