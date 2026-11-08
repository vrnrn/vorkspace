//! The vector face over a cached body.
use super::*;
use gpui::{TestAppContext, VisualTestContext, point, size};

struct Stage {
    look: BotLookConfig,
    anim: BotAnimState,
}
impl gpui::Render for Stage {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        let look = self.look.clone();
        let anim = self.anim;
        canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                let bounds = Bounds::new(point(px(10.), px(10.)), size(px(96.), px(96.)));
                paint_bot(bounds, &look, false, anim, window, cx);
            },
        )
        .size_full()
    }
}

fn look(shape: &str, face: &str, one_eye: bool) -> BotLookConfig {
    BotLookConfig {
        shape: shape.into(),
        pattern: "dither",
        print: "ground",
        face: face.into(),
        one_eye,
        textured: true,
        seed: 12345,
        grid: 4,
        bg_color: 0x3b82f6,
        fg_color: 0x0b1020,
    }
}

/// Quads painted in the face's ink, and in the body colour (visor slots,
/// ring centres).
fn face_quads(visual: &mut VisualTestContext, look: &BotLookConfig) -> (usize, usize) {
    let ink = gpui::Background::from(rgb_hsla(look.fg_color));
    let body = gpui::Background::from(rgb_hsla(look.bg_color));
    visual.update(|window, _| {
        let quads = window.painted_quads();
        (
            quads.iter().filter(|q| q.background == ink).count(),
            quads.iter().filter(|q| q.background == body).count(),
        )
    })
}

#[gpui::test]
fn every_face_draws_one_eye_or_a_pair_on_every_shape(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|_| Stage {
                look: look("squircle", "pills", false),
                anim: BotAnimState::default(),
            })
        })
        .unwrap()
    });
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let poses = [
        BotAnimState::default(),
        BotAnimState {
            blink: 0.1,
            ..Default::default()
        },
        BotAnimState {
            yaw: 0.6,
            pitch: -0.4,
            wide: 1.,
            squash_x: 1.05,
            squash_y: 0.95,
            hop_y: -0.03,
            ..Default::default()
        },
    ];
    for shape in BADGE_SHAPES {
        for face in BOT_FACES {
            for one_eye in [false, true] {
                for anim in poses {
                    let look = look(shape, face, one_eye);
                    window
                        .update(visual, |stage, _, cx| {
                            stage.look = look.clone();
                            stage.anim = anim;
                            cx.notify();
                        })
                        .unwrap();
                    visual.run_until_parked();
                    visual.refresh().unwrap();
                    let eyes = if one_eye { 1 } else { 2 };
                    let (ink, body) = face_quads(visual, &look);
                    match face {
                        // One ink band with a slot per eye.
                        "visor" => {
                            assert_eq!(ink, 1, "{shape}/{face}");
                            assert_eq!(body, eyes, "{shape}/{face}");
                        }
                        // A ring shows its centre unless it is mid-blink.
                        "rings" => {
                            assert_eq!(ink, eyes, "{shape}/{face}");
                            let centres = if anim.blink > 0.4 { eyes } else { 0 };
                            assert_eq!(body, centres, "{shape}/{face}");
                        }
                        _ => assert_eq!(ink, eyes, "{shape}/{face} one_eye={one_eye}"),
                    }
                }
            }
        }
    }
}

#[test]
fn legacy_expressions_read_as_styles_and_every_style_is_a_face() {
    use vorkspace_core::project_appearance::normalize_bot_face;
    // Saved Bots that chose an expression keep a real eye style.
    assert_eq!(normalize_bot_face("carets"), "slits");
    assert_eq!(normalize_bot_face("wink"), "rings");
    for face in BOT_FACES {
        assert_eq!(normalize_bot_face(face), face);
    }
}
