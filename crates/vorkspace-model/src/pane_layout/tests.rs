use super::*;

#[test]
fn minimum_width_and_stack_follow_nested_geometry_and_exclude_file_panes() {
    let tree = leaf(1)
        .split_pane(1, SplitDir::Right, 2)
        .split_pane(1, SplitDir::Down, 3)
        .split_pane(3, SplitDir::Down, -1)
        .split_pane(2, SplitDir::Down, 4);
    assert_eq!(tree.max_session_stack(|id| id > 0), 2);
    assert_eq!(tree.max_session_stack(|_| true), 3);
    assert!((tree.width_for_minimum(420.) - 840.).abs() < 0.01);
    let four = tree
        .split_pane(3, SplitDir::Down, 5)
        .split_pane(5, SplitDir::Down, 6);
    assert_eq!(four.max_session_stack(|id| id > 0), 4);
    assert_eq!(tree.leaf_ids(), [1, 3, -1, 2, 4]);
}

#[test]
fn fitting_makes_room_without_changing_vertical_splits_or_pane_identity() {
    let original = leaf(1)
        .split_pane(1, SplitDir::Right, 2)
        .split_pane(2, SplitDir::Down, 3)
        .place_pane(4, 1, Edge::Right);
    assert_eq!(original.minimum_width(420.), 1260.);
    assert!(original.width_for_minimum(420.) > 1500.);
    let mut fitted = original.clone();
    fitted.fit_width(1500., 420.);
    assert!(fitted.width_for_minimum(420.) <= 1500.01);
    assert_eq!(fitted.leaf_ids(), original.leaf_ids());
    let stacked: Vec<_> = fitted
        .leaves()
        .into_iter()
        .filter(|p| p.id == 2 || p.id == 3)
        .collect();
    assert_eq!(stacked[0].rect.h, 0.5);
    assert_eq!(stacked[1].rect.h, 0.5);
}

fn ids(node: &PaneNode) -> Vec<i64> {
    node.leaf_ids()
}
fn rects(node: &PaneNode) -> Vec<[f32; 4]> {
    node.leaves()
        .iter()
        .map(|l| [l.rect.x, l.rect.y, l.rect.w, l.rect.h])
        .collect()
}
fn close(a: &[[f32; 4]], b: &[[f32; 4]]) {
    assert_eq!(a.len(), b.len());
    for (a, b) in a.iter().zip(b) {
        for (x, y) in a.iter().zip(b) {
            assert!((x - y).abs() < 1e-5, "{a:?} != {b:?}");
        }
    }
}

#[test]
fn sash_sizes_move_only_adjacent_panes_and_clamp() {
    let r = sizes_at_boundary(&[0.2, 0.3, 0.5], 1, 0.7, MIN_SIZE);
    assert!((r[0] - 0.2).abs() < 1e-5 && (r[1] - 0.5).abs() < 1e-5);
    assert!((r[2] - 0.3).abs() < 1e-5);
    let r = sizes_at_boundary(&[0.5, 0.5], 0, 0.99, MIN_SIZE);
    assert!((r[0] - 0.92).abs() < 1e-5);
    let r = sizes_at_boundary(&[0.5, 0.5], 0, 0.01, MIN_SIZE);
    assert!((r[0] - 0.08).abs() < 1e-5);
    assert_eq!(
        sizes_at_boundary(&[0.5, 0.5], 2, 0.5, MIN_SIZE),
        vec![0.5, 0.5]
    );
}

#[test]
fn splits_share_a_group_and_report_their_sash() {
    let tree = leaf(1).split_pane(1, SplitDir::Right, 2);
    close(&rects(&tree), &[[0., 0., 0.5, 1.], [0.5, 0., 0.5, 1.]]);
    let sashes = tree.sashes();
    assert_eq!(sashes.len(), 1);
    assert_eq!((sashes[0].index, sashes[0].dir), (0, SplitDir::Right));
    assert_eq!(sashes[0].group, Rect::FULL);
    let three = tree.split_pane(2, SplitDir::Right, 3);
    close(
        &rects(&three),
        &[
            [0., 0., 1. / 3., 1.],
            [1. / 3., 0., 1. / 3., 1.],
            [2. / 3., 0., 1. / 3., 1.],
        ],
    );
    let nested = tree.split_pane(2, SplitDir::Down, 3);
    close(
        &rects(&nested),
        &[[0., 0., 0.5, 1.], [0.5, 0., 0.5, 0.5], [0.5, 0.5, 0.5, 0.5]],
    );
    let mut resized = nested.clone();
    resized.place_sash(&[1], 0, 0.25);
    close(
        &rects(&resized),
        &[
            [0., 0., 0.5, 1.],
            [0.5, 0., 0.5, 0.25],
            [0.5, 0.25, 0.5, 0.75],
        ],
    );
}

#[test]
fn edges_come_from_the_pane_centre() {
    let r = Rect {
        x: 0.,
        y: 0.,
        w: 100.,
        h: 100.,
    };
    assert_eq!(edge_from_point(20., 50., r), Edge::Left);
    assert_eq!(edge_from_point(80., 50., r), Edge::Right);
    assert_eq!(edge_from_point(50., 20., r), Edge::Top);
    assert_eq!(edge_from_point(50., 80., r), Edge::Bottom);
}

#[test]
fn move_pane_reorders_stacks_and_nests() {
    let row = leaf(1).split_pane(1, SplitDir::Right, 2);
    let next = row.move_pane(1, 2, Edge::Right);
    assert_eq!(ids(&next), [2, 1]);
    close(&rects(&next), &[[0., 0., 0.5, 1.], [0.5, 0., 0.5, 1.]]);

    let next = row.move_pane(2, 1, Edge::Bottom);
    assert_eq!(ids(&next), [1, 2]);
    close(&rects(&next), &[[0., 0., 1., 0.5], [0., 0.5, 1., 0.5]]);

    let three = row.split_pane(2, SplitDir::Right, 3);
    let next = three.move_pane(3, 1, Edge::Bottom);
    assert_eq!(ids(&next), [1, 3, 2]);
    close(
        &rects(&next),
        &[[0., 0., 0.5, 0.5], [0., 0.5, 0.5, 0.5], [0.5, 0., 0.5, 1.]],
    );

    let stacked = leaf(1).split_pane(1, SplitDir::Down, 2);
    let mixed = stacked.split_pane(1, SplitDir::Right, 3);
    let next = mixed.move_pane(3, 1, Edge::Bottom);
    assert_eq!(ids(&next), [1, 3, 2]);
    close(
        &rects(&next),
        &[[0., 0., 1., 0.25], [0., 0.25, 1., 0.25], [0., 0.5, 1., 0.5]],
    );
}

#[test]
fn place_pane_splits_inserts_moves_and_ignores_bad_targets() {
    let right = leaf(1).place_pane(2, 1, Edge::Right);
    assert_eq!(ids(&right), [1, 2]);
    assert_eq!(ids(&leaf(1).place_pane(2, 1, Edge::Left)), [2, 1]);
    let below = leaf(1).place_pane(2, 1, Edge::Bottom);
    close(&rects(&below)[1..], &[[0., 0.5, 1., 0.5]]);

    let row = leaf(1).split_pane(1, SplitDir::Right, 2);
    assert_eq!(ids(&row.place_pane(3, 1, Edge::Right)), [1, 3, 2]);
    let nested = row.place_pane(3, 1, Edge::Bottom);
    assert_eq!(ids(&nested), [1, 3, 2]);
    close(
        &rects(&nested),
        &[[0., 0., 0.5, 0.5], [0., 0.5, 0.5, 0.5], [0.5, 0., 0.5, 1.]],
    );
    assert_eq!(ids(&row.place_pane(1, 2, Edge::Right)), [2, 1]);
    assert_eq!(leaf(1).place_pane(2, 9, Edge::Right), leaf(1));
    assert_eq!(leaf(1).place_pane(1, 1, Edge::Right), leaf(1));
}

#[test]
fn removing_collapses_and_focus_moves_to_a_neighbour() {
    let tree = leaf(1)
        .split_pane(1, SplitDir::Right, 2)
        .split_pane(2, SplitDir::Down, 3);
    assert_eq!(tree.sibling(3), Some(2));
    assert_eq!(tree.sibling(1), Some(2));
    let tree = tree.remove(2).unwrap();
    assert_eq!(ids(&tree), [1, 3]);
    close(&rects(&tree), &[[0., 0., 0.5, 1.], [0.5, 0., 0.5, 1.]]);
    assert_eq!(tree.remove(3).unwrap(), leaf(1));
    assert_eq!(leaf(1).remove(1), None);
}

#[test]
fn neighbours_prefer_shared_edges() {
    let tree = leaf(1)
        .split_pane(1, SplitDir::Right, 2)
        .split_pane(2, SplitDir::Down, 3);
    assert_eq!(tree.neighbor(1, Edge::Right), Some(2));
    assert_eq!(tree.neighbor(3, Edge::Left), Some(1));
    assert_eq!(tree.neighbor(2, Edge::Bottom), Some(3));
    assert_eq!(tree.neighbor(3, Edge::Top), Some(2));
    assert_eq!(tree.neighbor(1, Edge::Left), None);
}

#[test]
fn serialized_trees_round_trip_and_restore_prunes_leaves() {
    let tree = leaf(1)
        .split_pane(1, SplitDir::Right, 2)
        .split_pane(2, SplitDir::Down, 3);
    let json = serde_json::to_string(&tree).unwrap();
    assert_eq!(serde_json::from_str::<PaneNode>(&json).unwrap(), tree);
    let pruned = tree.retain(&mut |id| id != 2).unwrap();
    assert_eq!(ids(&pruned), [1, 3]);
    assert_eq!(tree.retain(&mut |id| id == 3).unwrap(), leaf(3));
}
