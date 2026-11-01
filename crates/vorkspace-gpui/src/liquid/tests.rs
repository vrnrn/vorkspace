use super::*;

fn bounds(y: f32) -> Bounds<Pixels> {
    Bounds::new(point(px(8.), px(y)), size(px(200.), px(32.)))
}

/// Lays out rows "a" at 0 and "b" at 100, with `selected` selected.
fn frame(state: &mut State, selected: &str, mouse: Option<f32>, now: Instant) -> bool {
    state.rows = ["a", "b"]
        .into_iter()
        .zip([0., 100.])
        .map(|(key, y)| Row {
            key: key.into(),
            bounds: bounds(y),
            clip: Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(1000.))),
            selected: key == selected,
        })
        .collect();
    state.update(mouse.map(|y| point(px(20.), px(y))), now)
}

fn out(state: &State, key: &str, now: Instant) -> f32 {
    state
        .notches
        .iter()
        .find(|n| n.key.as_ref() == key)
        .map_or(0., |n| n.out.value(now))
}

#[test]
fn hover_pours_a_pill_that_a_click_keeps_while_the_old_one_drains() {
    let mut state = State::default();
    let start = Instant::now();
    assert!(
        !frame(&mut state, "a", None, start),
        "the first paint never animates"
    );
    assert_eq!(out(&state, "a", start), 1.);

    // Hovering "b" pours its pill out of the pane.
    assert!(frame(&mut state, "a", Some(110.), start));
    let mid = start + POUR / 2;
    assert!(frame(&mut state, "a", Some(110.), mid));
    assert!(out(&state, "b", mid) > 0. && out(&state, "b", mid) < 1.);
    let poured = start + POUR;
    assert!(!frame(&mut state, "a", Some(110.), poured));
    assert_eq!(out(&state, "b", poured), 1.);

    // Clicking it keeps "b" out; only "a" moves, back into the pane.
    assert!(frame(&mut state, "b", Some(110.), poured));
    assert_eq!(out(&state, "b", poured), 1.);
    // The click also grows "b"'s look in, which takes as long as a pour.
    let drained = poured + POUR;
    assert!(!frame(&mut state, "b", Some(110.), drained));
    assert_eq!(out(&state, "a", drained), 0.);
    assert!(state.notches.iter().all(|n| n.key.as_ref() == "b"));
}

#[test]
fn leaving_an_unselected_row_drains_it_from_where_it_was() {
    let mut state = State::default();
    let start = Instant::now();
    frame(&mut state, "a", None, start);
    frame(&mut state, "a", Some(110.), start);
    let mid = start + POUR / 2;
    frame(&mut state, "a", Some(110.), mid);
    let partial = out(&state, "b", mid);
    frame(&mut state, "a", None, mid);
    assert_eq!(out(&state, "b", mid), partial);
    assert!(!frame(&mut state, "a", None, mid + DRAIN));
    assert!(state.notches.iter().all(|n| n.key.as_ref() == "a"));
}

#[gpui::test]
fn a_rows_padding_does_not_shift_its_pill(cx: &mut gpui::TestAppContext) {
    use gpui::prelude::*;
    struct Rail(Liquid);
    impl gpui::Render for Rail {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            gpui::div()
                .size_full()
                .relative()
                .child(self.0.surface(None, true))
                .child(
                    gpui::div()
                        .debug_selector(|| "row".into())
                        .relative()
                        .w(px(200.))
                        .h(px(32.))
                        .px_2()
                        .py(px(6.))
                        .child(self.0.row("row", true)),
                )
        }
    }
    let liquid = Liquid::default();
    let window = cx.add_window(|_, _| Rail(liquid.clone()));
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let row = visual.debug_bounds("row").unwrap();
    assert_eq!(liquid.0.borrow().notches[0].bounds, row);
}

#[test]
fn neighbouring_pills_stay_apart_without_overlapping_joins() {
    let mut state = State::default();
    let now = Instant::now();
    state.rows = ["a", "b"]
        .into_iter()
        .zip([0., 33.])
        .map(|(key, y)| Row {
            key: key.into(),
            bounds: bounds(y),
            clip: Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(1000.))),
            selected: key == "a",
        })
        .collect();
    state.update(None, now);
    state.update(Some(point(px(20.), px(40.))), now);
    let later = now + POUR;
    state.update(Some(point(px(20.), px(40.))), later);
    let shapes = state.shapes(240., later);
    assert_eq!(shapes.len(), 2);
    let (above, below) = (shapes[0].1, shapes[1].1);
    assert!(!above.join_bottom && !below.join_top);
    assert!(above.bottom < below.top, "neighbours never overlap");
    assert!(above.join_top && below.join_bottom);
}

#[test]
fn a_surface_below_its_rows_works_in_swapped_axes() {
    let b = Bounds::new(point(px(3.), px(5.)), size(px(7.), px(11.)));
    assert_eq!(flip(true, flip(true, b)), b);
    assert_eq!(flip(true, b).origin, point(px(5.), px(3.)));
    assert_eq!(flip(false, b), b);
}

#[test]
fn an_idle_rail_requests_no_frames() {
    let mut state = State::default();
    let now = Instant::now();
    frame(&mut state, "a", None, now);
    for _ in 0..3 {
        assert!(!frame(&mut state, "a", None, now));
    }
    let shape = Shape {
        left: 8.,
        top: 0.,
        bottom: 32.,
        join: RADIUS,
        join_top: true,
        join_bottom: true,
    };
    assert!(outline(shape, 240., false, PathBuilder::fill()).is_some());
    let empty = Shape {
        left: 240.,
        ..shape
    };
    assert!(outline(empty, 240., false, PathBuilder::fill()).is_none());
}
