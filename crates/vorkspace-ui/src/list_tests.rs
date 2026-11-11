use gpui::{
    Context, ListAlignment, ListOffset, ListSizingBehavior, ListState, Pixels, Render, ScrollDelta,
    ScrollWheelEvent, TestAppContext, TouchPhase, UniformListScrollHandle, VisualTestContext,
    Window, div, list, point, prelude::*, px, uniform_list,
};
use std::{cell::RefCell, ops::Range, rc::Rc};

struct VariableRows {
    state: ListState,
    height: Pixels,
}

impl Render for VariableRows {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let height = self.height;
        div().size_full().child(
            list(self.state.clone(), move |_, _, _| {
                div().h(height).into_any_element()
            })
            .w(px(300.))
            .h(px(150.)),
        )
    }
}

#[gpui::test]
fn list_preserves_a_remeasured_anchor_across_prepend_and_reports_the_applied_wheel(
    cx: &mut TestAppContext,
) {
    let state = ListState::new(20, ListAlignment::Top, px(200.));
    let event = Rc::new(RefCell::new(None::<Range<usize>>));
    state.set_scroll_handler({
        let event = event.clone();
        move |scrolled, _, _| *event.borrow_mut() = Some(scrolled.visible_range.clone())
    });
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|_| VariableRows {
                state: state.clone(),
                height: px(50.),
            })
        })
        .unwrap()
    });
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |_, _, cx| {
            state.scroll_to(ListOffset {
                item_ix: 3,
                offset_in_item: px(20.),
            });
            cx.notify();
        })
        .unwrap();
    visual.refresh().unwrap();
    assert_eq!(state.bounds_for_item(3).unwrap().size.height, px(50.));
    window
        .update(visual, |view, _, cx| {
            state.remeasure();
            state.splice(0..0, 1);
            view.height = px(100.);
            cx.notify();
        })
        .unwrap();
    visual.refresh().unwrap();
    let anchor = state.logical_scroll_top();
    assert_eq!((anchor.item_ix, anchor.offset_in_item), (4, px(40.)));
    visual.simulate_event(ScrollWheelEvent {
        position: point(px(30.), px(60.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(-110.))),
        modifiers: Default::default(),
        touch_phase: TouchPhase::Moved,
    });
    let applied = state.logical_scroll_top();
    assert_eq!(applied.item_ix, 5);
    assert_eq!(event.borrow().as_ref().unwrap().start, applied.item_ix);
}

struct UniformRows {
    handle: UniformListScrollHandle,
    count: usize,
    sizing: ListSizingBehavior,
    frame_measures: Rc<RefCell<Vec<usize>>>,
}

impl Render for UniformRows {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.frame_measures.borrow_mut().push(0);
        let measures = self.frame_measures.clone();
        div().size_full().child(
            uniform_list("uniform-fixture", self.count, move |range, _, _| {
                if range == (12..13) {
                    *measures.borrow_mut().last_mut().unwrap() += 1;
                }
                range
                    .map(|ix| div().h(px(28.)).child(ix.to_string()))
                    .collect()
            })
            .with_sizing_behavior(self.sizing)
            .with_width_from_item(Some(12))
            .track_scroll(&self.handle)
            .y_flipped(true)
            .w(px(240.))
            .h(px(96.)),
        )
    }
}

#[gpui::test]
fn uniform_list_measures_once_and_keeps_its_visual_bottom_when_flipped(cx: &mut TestAppContext) {
    for sizing in [ListSizingBehavior::Auto, ListSizingBehavior::Infer] {
        let handle = UniformListScrollHandle::new();
        let measures = Rc::new(RefCell::new(Vec::new()));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| UniformRows {
                    handle: handle.clone(),
                    count: 20,
                    sizing,
                    frame_measures: measures.clone(),
                })
            })
            .unwrap()
        });
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        window
            .update(visual, |_, _, cx| {
                handle.scroll_to_bottom();
                cx.notify();
            })
            .unwrap();
        visual.refresh().unwrap();
        assert_eq!(measures.borrow().last(), Some(&1));
        assert_eq!(handle.is_scrolled_to_end(), Some(true));
        window
            .update(visual, |view, _, cx| {
                view.count = 1;
                cx.notify();
            })
            .unwrap();
        visual.refresh().unwrap();
        assert_eq!(handle.0.borrow().base_handle.offset().y, px(0.));
    }
}
