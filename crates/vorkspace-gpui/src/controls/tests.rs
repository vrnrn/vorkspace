use super::*;
use crate::input::Input;
use gpui::{
    Element, Entity, Focusable, InputEvent, TestAppContext, VisualTestContext, WindowHandle, size,
};
use vorkspace_ui::theme;

/// GPUI's simulate_keystrokes emits only key-down; native buttons click on key-up.
pub(crate) fn press_key(visual: &mut VisualTestContext, key: &str) {
    visual.update(|window, cx| {
        let keystroke = gpui::Keystroke::parse(key).unwrap();
        window.dispatch_event(
            gpui::KeyDownEvent {
                keystroke: keystroke.clone(),
                is_held: false,
                prefer_character_input: false,
            }
            .to_platform_input(),
            cx,
        );
        window.dispatch_event(gpui::KeyUpEvent { keystroke }.to_platform_input(), cx);
    });
    visual.run_until_parked();
    visual.refresh().unwrap();
}

fn node(control: Stateful<Div>) -> gpui::accesskit::Node {
    let mut node = gpui::accesskit::Node::new(control.a11y_role().unwrap());
    control.write_a11y_info(&mut node);
    node
}

#[test]
fn shared_controls_expose_names_and_switch_states_without_selection() {
    for on in [false, true] {
        for control in [
            switch("texture", "Texture", on, true),
            compact_switch("automation", "Enable automation", on),
        ] {
            let node = node(control);
            assert_eq!(node.role(), Role::Switch);
            assert!(node.label().is_some());
            assert_eq!(
                node.toggled(),
                Some(if on {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
            );
            assert_eq!(node.is_selected(), None);
        }
    }
    let node = node(button("save", "Save", true));
    assert_eq!(node.role(), Role::Button);
    assert_eq!(node.label(), Some("Save"));
}

struct Surface {
    on: bool,
    compact: bool,
    enabled: bool,
    changes: usize,
    actions: usize,
    draft: Entity<Input>,
}
impl Render for Surface {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let toggle = if self.compact {
            compact_switch("switch", "Texture", self.on)
        } else {
            switch("switch", "Texture", self.on, self.enabled)
        };
        div()
            .size_full()
            .bg(shell_fill())
            .p_4()
            .flex()
            .flex_col()
            .items_start()
            .gap_3()
            .child(
                toggle
                    .debug_selector(|| "switch".into())
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.enabled {
                            this.on = !this.on;
                            this.changes += 1;
                            cx.notify();
                        }
                    })),
            )
            .child(
                button("disabled", "Disabled", false)
                    .h(px(24.))
                    .child("Disabled"),
            )
            .child(
                icon_button("action", "Save draft", true)
                    .debug_selector(|| "action".into())
                    .hover(|s| s.bg(content(0.08)))
                    .child(vorkspace_ui::theme::icon("check", 14.))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.actions += 1;
                        cx.notify();
                    })),
            )
            .child(div().w(px(240.)).h(px(32.)).child(self.draft.clone()))
    }
}
fn surface(cx: &mut TestAppContext) -> WindowHandle<Surface> {
    cx.update(|cx| {
        crate::input::bind(cx);
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let draft = cx.new(|cx| {
                    let mut input = Input::field("Draft", "Draft", content(0.35), cx);
                    input.set_text("Retain this unsent draft".into(), cx);
                    input
                });
                Surface {
                    on: false,
                    compact: false,
                    enabled: true,
                    changes: 0,
                    actions: 0,
                    draft,
                }
            })
        })
        .unwrap()
    })
}

#[gpui::test]
fn keyboard_activation_skips_disabled_controls_and_keeps_drafts(cx: &mut TestAppContext) {
    let window = surface(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    window
        .update(visual, |_, window, cx| window.focus_next(cx))
        .unwrap();
    visual.run_until_parked();
    // Paint the new focus owner before dispatching its press/release pair.
    visual.refresh().unwrap();
    for key in ["space", "enter", "cmd-enter"] {
        press_key(visual, key);
    }
    visual.run_until_parked();
    window
        .update(visual, |view, window, cx| {
            assert_eq!(
                view.changes, 2,
                "each unmodified press/release activates once"
            );
            assert!(!view.on);
            assert_eq!(view.draft.read(cx).content, "Retain this unsent draft");
            window.focus_next(cx);
        })
        .unwrap();
    visual.run_until_parked();
    // Paint the new focus owner before dispatching its press/release pair.
    visual.refresh().unwrap();
    for key in ["space", "enter"] {
        press_key(visual, key);
    }
    window
        .update(visual, |view, window, cx| {
            assert_eq!(view.actions, 2, "disabled text control was skipped");
            assert_eq!(view.changes, 2);
            window.focus_prev(cx);
        })
        .unwrap();
    visual.run_until_parked();
    // Paint the new focus owner before dispatching its press/release pair.
    visual.refresh().unwrap();
    press_key(visual, "space");
    window
        .update(visual, |view, _, _| {
            assert!(view.on);
            assert_eq!(view.changes, 3);
        })
        .unwrap();
    window
        .update(visual, |view, window, cx| {
            view.enabled = false;
            // Move into the retained input first, then restart traversal.
            window.focus(&view.draft.focus_handle(cx), cx);
            cx.notify();
        })
        .unwrap();
    visual.run_until_parked();
    window
        .update(visual, |_, window, cx| window.focus_next(cx))
        .unwrap();
    visual.run_until_parked();
    // Paint the new focus owner before dispatching its press/release pair.
    visual.refresh().unwrap();
    press_key(visual, "space");
    window
        .update(visual, |view, _, cx| {
            assert_eq!(
                view.actions, 3,
                "disabled switch is skipped after an enabled-to-disabled transition"
            );
            assert_eq!(view.changes, 3);
            assert_eq!(view.draft.read(cx).content, "Retain this unsent draft");
        })
        .unwrap();
}

#[gpui::test]
fn switch_geometry_and_focus_feedback_follow_theme_and_state(cx: &mut TestAppContext) {
    let window = surface(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    for light in [false, true] {
        for workspace_theme in ["default", "harbor"] {
            visual.update(|_, _| {
                theme::configure(
                    &crate::settings::Preferences {
                        theme: if light { "light" } else { "dark" }.into(),
                        workspace_theme: workspace_theme.into(),
                        ..Default::default()
                    },
                    light,
                    true,
                );
            });
            for compact in [false, true] {
                for on in [false, true] {
                    window
                        .update(visual, |view, window, cx| {
                            view.compact = compact;
                            view.on = on;
                            window.blur(cx);
                            cx.notify();
                        })
                        .unwrap();
                    visual.run_until_parked();
                    visual.refresh().unwrap();
                    let bounds = visual.debug_bounds("switch").unwrap();
                    let (width, height, knob) = if compact {
                        (28., 16., 12.)
                    } else {
                        (36., 20., 16.)
                    };
                    assert_eq!(bounds.size, size(px(width), px(height)));
                    let knob_bounds = visual.update(|window, _| {
                        let knob_quad = window
                            .painted_quads()
                            .into_iter()
                            .find(|q| {
                                q.background == gpui::Background::from(gpui::white())
                                    && q.bounds.size.width.0 == knob * window.scale_factor()
                            })
                            .expect("white switch knob");
                        knob_quad.bounds
                    });
                    assert_eq!(
                        knob_bounds.origin.x.0,
                        (bounds.left().as_f32() + if on { width - knob - 2. } else { 2. })
                            * visual.update(|w, _| w.scale_factor())
                    );
                    assert_eq!(
                        knob_bounds.origin.y.0,
                        (bounds.top().as_f32() + 2.) * visual.update(|w, _| w.scale_factor())
                    );
                    window
                        .update(visual, |_, window, cx| window.focus_next(cx))
                        .unwrap();
                    visual.run_until_parked();
                    visual.simulate_keystrokes("right");
                    visual.refresh().unwrap();
                    assert_eq!(visual.debug_bounds("switch").unwrap(), bounds);
                    visual.update(|window, _| {
                        let expected = if on {
                            accent_color().opacity(0.65)
                        } else {
                            content(0.4)
                        };
                        assert!(
                            window
                                .painted_quads()
                                .iter()
                                .any(|q| q.background == gpui::Background::from(expected)),
                            "keyboard focus has a visible theme-aware fill"
                        );
                        assert!(
                            window
                                .painted_quads()
                                .iter()
                                .any(|q| q.bounds == knob_bounds
                                    && q.background == gpui::Background::from(gpui::white())),
                            "focus must not move the knob"
                        );
                    });
                }
            }
        }
    }
    visual.update(|_, _| {
        theme::configure(&Default::default(), false, true);
    });
}

#[test]
#[ignore = "Requires Metal and VOKKY_COMPONENT_CAPTURE_DIR"]
fn native_control_captures() {
    use std::sync::Arc;
    let output = std::path::PathBuf::from(std::env::var_os("VOKKY_COMPONENT_CAPTURE_DIR").unwrap());
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
        cx.update(|_| {
            theme::configure(
                &crate::settings::Preferences {
                    theme: if light { "light" } else { "dark" }.into(),
                    workspace_theme: "harbor".into(),
                    ..Default::default()
                },
                light,
                true,
            );
        });
        for compact in [false, true] {
            for on in [false, true] {
                let window = cx
                    .open_window(size(px(320.), px(200.)), |_, cx| {
                        cx.new(|cx| {
                            let draft = cx.new(|cx| {
                                let mut input = Input::field("Draft", "Draft", content(0.35), cx);
                                input.set_text("Retained draft".into(), cx);
                                input
                            });
                            Surface {
                                on,
                                compact,
                                enabled: true,
                                changes: 0,
                                actions: 0,
                                draft,
                            }
                        })
                    })
                    .unwrap();
                cx.run_until_parked();
                for focused in [false, true] {
                    if focused {
                        cx.update_window(window.into(), |_, window, cx| {
                            window.focus_next(cx);
                            window.dispatch_event(
                                gpui::KeyDownEvent {
                                    keystroke: gpui::Keystroke::parse("right").unwrap(),
                                    is_held: false,
                                    prefer_character_input: false,
                                }
                                .to_platform_input(),
                                cx,
                            );
                        })
                        .unwrap();
                        cx.run_until_parked();
                    }
                    cx.capture_screenshot(window.into())
                        .unwrap()
                        .save(output.join(format!(
                            "controls-{}-{}-{}-{}.png",
                            if light { "light" } else { "dark" },
                            if compact { "compact" } else { "standard" },
                            if on { "on" } else { "off" },
                            if focused { "focus" } else { "rest" },
                        )))
                        .unwrap();
                }
                cx.update_window(window.into(), |_, window, _| window.remove_window())
                    .unwrap();
            }
        }
    }
    cx.update(|_| {
        theme::configure(&Default::default(), false, true);
    });
}

#[gpui::test]
fn icon_tooltip_preserves_geometry_and_mouse_click_routing(cx: &mut TestAppContext) {
    let window = surface(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let action = visual.debug_bounds("action").unwrap();
    visual.simulate_mouse_move(action.center(), None, Default::default());
    visual.run_until_parked();
    visual
        .executor()
        .advance_clock(std::time::Duration::from_secs(1));
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("control-tooltip").is_some());
    assert_eq!(visual.debug_bounds("action").unwrap(), action);
    window
        .update(visual, |view, window, cx| {
            window.focus(&view.draft.focus_handle(cx), cx)
        })
        .unwrap();
    visual.run_until_parked();
    visual.simulate_click(action.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |view, window, cx| {
            assert_eq!(view.actions, 1);
            assert!(
                view.draft.focus_handle(cx).is_focused(window),
                "mouse action retains the draft's focus"
            );
        })
        .unwrap();
    visual.simulate_mouse_move(gpui::point(px(300.), px(300.)), None, Default::default());
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("control-tooltip").is_none());
}
