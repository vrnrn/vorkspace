use super::*;
use std::collections::HashSet;

#[test]
fn minion_looks_survive_parent_edits_and_roster_reload() {
    let mut parent = Bot {
        name: "Parent".into(),
        color: 0x3b82f6,
        fg_color: Some(0x0b1528),
        ..Default::default()
    };
    let before = serde_json::to_value(&parent).unwrap();
    let look = minion_look(&parent, "hello");
    assert_eq!(
        serde_json::to_value(&parent).unwrap(),
        before,
        "deriving a look never changes its parent"
    );
    assert!(look.one_eye && look.textured);
    assert_eq!(look.seed, 61723, "freeze the existing stable identity seed");
    let restored: Bot = serde_json::from_str(&serde_json::to_string(&parent).unwrap()).unwrap();
    assert_eq!(minion_look(&restored, "hello"), look);

    // Mutable parent identity and appearance must not reroll a team member.
    parent.name = "Renamed".into();
    parent.id = "another-parent-record".into();
    parent.mascot = "wink".into();
    parent.shape = "hex".into();
    parent.textured = false;
    parent.seed = Some(54321);
    parent.grid = Some(8);
    assert_eq!(minion_look(&parent, "hello"), look);

    parent.color = 0x22c55e;
    parent.fg_color = None;
    assert_eq!(
        minion_look(&parent, "hello"),
        BotLookConfig {
            pattern: "dither",
            print: "ground",
            bg_color: 0x22c55e,
            fg_color: default_foreground_color(0x22c55e),
            ..look.clone()
        }
    );
    parent.fg_color = Some(0x052e16);
    assert_eq!(minion_look(&parent, "hello").fg_color, 0x052e16);
}

#[test]
fn minion_identities_vary_existing_faces_bodies_and_patterns_independently() {
    let parent = Bot::default();
    let mut faces = HashSet::new();
    let mut shapes = HashSet::new();
    let mut pairs = HashSet::new();
    let mut seeds = HashSet::new();
    let mut grids = HashSet::new();
    for i in 0..256 {
        let look = minion_look(&parent, &format!("persistent-member-{i}"));
        assert!(BOT_FACES.contains(&look.face.as_str()));
        assert!(BADGE_SHAPES.contains(&look.shape.as_str()));
        assert!((2..=8).contains(&look.grid));
        assert!((10000..=99999).contains(&look.seed));
        pairs.insert((look.face.clone(), look.shape.clone()));
        faces.insert(look.face);
        shapes.insert(look.shape);
        seeds.insert(look.seed);
        grids.insert(look.grid);
    }
    assert_eq!(faces.len(), BOT_FACES.len());
    assert_eq!(shapes.len(), BADGE_SHAPES.len());
    assert_eq!(grids.len(), 7);
    assert!(
        pairs.len() >= 20,
        "body and face must not be locked together"
    );
    assert!(
        seeds.len() > 240,
        "member patterns must not collapse to a few looks"
    );
}

#[test]
fn bot_and_legacy_factories_preserve_ordinary_looks() {
    let parent = Bot::default();
    let ordinary = BotLookConfig::from_bot(&parent);
    assert!(!ordinary.one_eye);
    assert_eq!(ordinary.face, parent.face_style());
    assert_eq!(ordinary.shape, parent.badge_shape());
    assert_eq!(ordinary.textured, parent.textured);
    assert_eq!(ordinary.seed, parent.resolved_seed());
    assert_eq!(ordinary.grid, parent.resolved_grid());
    assert_eq!(ordinary.bg_color, parent.color);
    assert_eq!(ordinary.fg_color, parent.resolved_fg_color());
    for mascot in vorkspace_core::project_appearance::MASCOTS {
        let legacy = BotLookConfig::from_legacy(mascot, 0x3b82f6);
        assert!(!legacy.one_eye);
        assert_eq!(legacy.face, normalize_bot_face(mascot));
        assert_eq!(legacy.shape, "squircle");
        assert!(legacy.textured);
        assert_eq!(legacy.grid, 4);
        assert_eq!(legacy.seed, default_seed_for(&format!("{mascot}:3b82f6")));
    }
}

#[test]
fn one_eye_and_paired_faces_share_the_body_until_its_look_changes() {
    let parent = Bot::default();
    let single = minion_look(&parent, "member");
    let paired = BotLookConfig {
        one_eye: false,
        face: "visor".into(),
        ..single.clone()
    };
    let key = |look| BodyKey::new(look, 48);
    assert!(key(&single) == key(&paired));
    let recolored = minion_look(
        &Bot {
            color: 0x22c55e,
            ..parent
        },
        "member",
    );
    assert!(key(&single) != key(&recolored));
}

struct TeamAvatars(&'static str);
impl gpui::Render for TeamAvatars {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        let parent = Bot::default();
        gpui::div().size_full().flex().children(
            (0..12).map(|i| minion_avatar(&parent, &format!("member-{i}"), self.0, 24., true)),
        )
    }
}

#[gpui::test]
fn minion_team_shares_existing_bounded_wakes_and_still_idle_sleeps(cx: &mut gpui::TestAppContext) {
    let _clock = vorkspace_ui::clock::use_test_clock_with_endless_frames(cx);
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |window, cx| {
            window.activate_window();
            cx.new(|_| TeamAvatars("idle"))
        })
        .unwrap()
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    PENDING_FRAMES.with(|pending| assert!(pending.borrow().is_empty()));
    window
        .update(cx, |team, _, cx| {
            team.0 = "working";
            cx.notify();
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    PENDING_FRAMES.with(|pending| {
        let pending = pending.borrow();
        assert_eq!(
            pending.len(),
            1,
            "all team members share their rendering owner's wake"
        );
        let wake = pending.values().next().unwrap();
        assert!(wake.deadline > Instant::now());
        assert!(wake.deadline <= Instant::now() + AVATAR_FRAME);
    });
    window
        .update(cx, |team, _, cx| {
            team.0 = "idle";
            cx.notify();
        })
        .unwrap();
    cx.background_executor.advance_clock(AVATAR_FRAME * 2);
    cx.run_until_parked();
    cx.refresh().unwrap();
    PENDING_FRAMES.with(|pending| assert!(pending.borrow().is_empty()));
}
