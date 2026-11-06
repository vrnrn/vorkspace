//! Regression coverage for nested frame caches and sidebar transitions.
use gpui::{
    Context, Entity, MouseButton, StyleRefinement, TestAppContext, Window, div, prelude::*, px,
};
use std::{cell::Cell, rc::Rc};

struct CachedRoot {
    child: Entity<CachedAction>,
}

impl Render for CachedRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.child
            .clone()
            .cached(StyleRefinement::default().size(px(100.)))
    }
}

struct CachedAction {
    renders: Rc<Cell<usize>>,
    clicks: Rc<Cell<usize>>,
}

impl Render for CachedAction {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        let clicks = self.clicks.clone();
        div()
            .id("cached-action")
            .size(px(40.))
            .on_mouse_down(MouseButton::Left, move |_, _, _| {
                clicks.set(clicks.get() + 1);
            })
    }
}

struct NestedCacheRoot {
    child: Entity<CachedRoot>,
    leading_hitboxes: usize,
    cached: bool,
}

impl Render for NestedCacheRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .children(
                (0..self.leading_hitboxes).map(|i| div().id(i).absolute().size(px(1.)).occlude()),
            )
            .child(if self.cached {
                self.child
                    .clone()
                    .cached(StyleRefinement::default().size(px(100.)))
                    .into_any_element()
            } else {
                self.child.clone().into_any_element()
            })
    }
}

#[gpui::test]
fn nested_cache_survives_switching_its_relocated_parent_to_uncached(cx: &mut TestAppContext) {
    let renders = Rc::new(Cell::new(0));
    let clicks = Rc::new(Cell::new(0));
    let window = cx.add_window(|_, cx| NestedCacheRoot {
        leading_hitboxes: 100,
        cached: true,
        child: cx.new(|cx| CachedRoot {
            child: cx.new(|_| CachedAction {
                renders: renders.clone(),
                clicks: clicks.clone(),
            }),
        }),
    });
    let draw = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .unwrap();
    };
    // Check both out-of-bounds ranges and old ranges that still fit but refer
    // to the wrong elements. Bounds checks alone cannot make reuse correct.
    for leading_hitboxes in [0, 200] {
        window
            .update(cx, |root, _, _| {
                root.leading_hitboxes = 100;
                root.cached = true;
            })
            .unwrap();
        draw(cx);
        let initial_renders = renders.get();
        // Replaying the parent relocates the child without visiting its cache.
        window
            .update(cx, |root, _, _| root.leading_hitboxes = leading_hitboxes)
            .unwrap();
        draw(cx);
        assert_eq!(renders.get(), initial_renders);
        // A sidebar transition renders the parent without caching. The child must
        // rebuild instead of interpreting its old ranges in the new frame.
        window.update(cx, |root, _, _| root.cached = false).unwrap();
        draw(cx);
        assert_eq!(renders.get(), initial_renders + 1);
        for _ in 0..3 {
            draw(cx);
            assert_eq!(renders.get(), initial_renders + 1);
        }
        let before_click = clicks.get();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.simulate_click(gpui::point(px(20.), px(20.)), Default::default());
        assert_eq!(clicks.get(), before_click + 1);
    }
}

#[gpui::test]
fn explorer_sidebar_survives_repeated_toggles(cx: &mut TestAppContext) {
    use crate::workspace::SidebarTab;

    vorkspace_ui::clock::use_test_clock(cx);
    let (window, _, active) = super::window(cx);
    window
        .update(cx, |shell, window, cx| {
            shell.workspace.project_rail_open = false;
            shell.select_classic_sidebar(SidebarTab::Files, window, cx);
            shell.toggle_sidebar(window, cx);
            shell.invalidate_chrome(cx);
            cx.notify();
        })
        .unwrap();
    vorkspace_ui::clock::settle_animations(window, cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    let draw = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        })
        .unwrap();
    };

    for _ in 0..3 {
        let trigger = visual.debug_bounds("classic-compact-tab-explorer").unwrap();
        visual.simulate_mouse_move(trigger.center(), None, Default::default());
        visual.run_until_parked();
        vorkspace_ui::clock::settle_animations(window, visual);
        assert!(visual.debug_bounds("sidebar-peek").is_none());
        assert!(visual.debug_bounds("sidebar-frame").is_none());
        visual.simulate_click(trigger.center(), Default::default());
        visual.run_until_parked();
        vorkspace_ui::clock::settle_animations(window, visual);
        assert!(visual.debug_bounds("sidebar-frame").is_some());
        draw(visual);
        let renders = window
            .update(visual, |shell, _, cx| shell.sidebar_frame.read(cx).renders)
            .unwrap();
        draw(visual);
        window
            .update(visual, |shell, _, cx| {
                assert_eq!(
                    shell.sidebar_frame.read(cx).renders,
                    renders,
                    "a settled sidebar still uses its cached drawing"
                );
            })
            .unwrap();

        window
            .update(visual, |shell, window, cx| shell.toggle_sidebar(window, cx))
            .unwrap();
        vorkspace_ui::clock::settle_animations(window, visual);
        assert!(visual.debug_bounds("sidebar-frame").is_none());
        window
            .update(visual, |shell, _, _| {
                assert_eq!(shell.workspace.active, Some(active));
                assert_eq!(shell.workspace.sidebar_tab(), SidebarTab::Files);
            })
            .unwrap();
        draw(visual);
    }
}
