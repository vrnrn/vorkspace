use super::*;
use PathCommand::{Line, Move};

fn c(sha: &str, parents: &[&str]) -> GraphCommit {
    GraphCommit {
        sha: sha.into(),
        parents: parents.iter().map(|p| p.to_string()).collect(),
        head: false,
        refs: Vec::new(),
    }
}
fn with_ref(mut commit: GraphCommit, name: &str, head: bool) -> GraphCommit {
    commit.refs.push((name.into(), "local".into()));
    commit.head = head;
    commit
}
fn ids(rows: &[Row]) -> Vec<(String, Vec<String>)> {
    rows.iter()
        .map(|r| {
            (
                r.sha.clone(),
                r.output.iter().map(|l| l.id.clone()).collect(),
            )
        })
        .collect()
}
fn expect(rows: &[Row], want: &[(&str, &[&str])]) {
    let want: Vec<(String, Vec<String>)> = want
        .iter()
        .map(|(s, o)| (s.to_string(), o.iter().map(|x| x.to_string()).collect()))
        .collect();
    assert_eq!(ids(rows), want);
}
fn paths(graph: &RowGraph) -> Vec<&[PathCommand]> {
    graph.paths.iter().map(|p| p.commands.as_slice()).collect()
}
fn arc(x: f32, y: f32, radius: f32, sweep: bool) -> PathCommand {
    PathCommand::Arc {
        x,
        y,
        radius,
        sweep,
    }
}
fn merge() -> Vec<Row> {
    layout(&[
        c("M", &["A", "F"]),
        c("A", &["B"]),
        c("F", &["B"]),
        c("B", &[]),
    ])
}

#[test]
fn linear_history_keeps_one_swimlane() {
    let rows = layout(&[c("c", &["b"]), c("b", &["a"]), c("a", &[])]);
    expect(&rows, &[("c", &["b"]), ("b", &["a"]), ("a", &[])]);
    assert_eq!(rows[1].input[0].id, "b");
    assert_eq!(rows[0].output[0].color, COLOR_REGISTRY[0]);
}
#[test]
fn merge_parent_opens_a_lane() {
    let rows = merge();
    expect(
        &rows,
        &[
            ("M", &["A", "F"]),
            ("A", &["B", "F"]),
            ("F", &["B", "B"]),
            ("B", &[]),
        ],
    );
    assert_eq!(
        rows[0].output.iter().map(|l| l.color).collect::<Vec<_>>(),
        COLOR_REGISTRY[..2]
    );
}
#[test]
fn second_child_forks_and_stash_index_keeps_through_lane() {
    let rows = layout(&[c("C", &["B"]), c("D", &["B"]), c("B", &[])]);
    expect(&rows, &[("C", &["B"]), ("D", &["B", "B"]), ("B", &[])]);
    let tip = row_graph(&rows[1], SWIMLANE_HEIGHT);
    assert_eq!(
        paths(&tip),
        [
            &[Move(11., 0.), Line(11., 22.)],
            &[Move(22., 11.), Line(22., 22.)]
        ]
    );

    let rows = layout(&[
        with_ref(c("A", &["M"]), "main", true),
        with_ref(c("S", &["W", "I"]), "stash", false),
        c("I", &["W"]),
        with_ref(c("W", &["F"]), "feature", false),
        c("F", &["M"]),
        c("M", &[]),
    ]);
    expect(
        &rows,
        &[
            ("A", &["M"]),
            ("S", &["M", "W", "I"]),
            ("I", &["M", "W", "W"]),
            ("W", &["M", "F"]),
            ("F", &["M", "M"]),
            ("M", &[]),
        ],
    );
    assert!(
        paths(&row_graph(&rows[2], SWIMLANE_HEIGHT))
            .contains(&&[Move(22., 0.), Line(22., 22.)][..])
    );
    assert!(
        paths(&row_graph(&rows[5], SWIMLANE_HEIGHT))
            .contains(&&[Move(22., 0.), arc(11., 11., 11., true), Line(11., 11.)][..])
    );
}
#[test]
fn current_branch_uses_the_ref_color() {
    let rows = layout(&[with_ref(c("c", &["b"]), "main", true), c("b", &[])]);
    assert_eq!(rows[0].kind, NodeKind::Head);
    assert_eq!(rows[0].output[0].color, HISTORY_ITEM_REF_COLOR);
    assert_eq!(rows[0].refs[0].color, Some(HISTORY_ITEM_REF_COLOR));
}
#[test]
fn taller_rows_extend_lanes_not_nodes() {
    let rows = merge();
    let m = row_graph(&rows[0], 44.);
    assert_eq!(m.height, 44.);
    assert_eq!(
        m.circles.iter().map(|c| (c.cy, c.r)).collect::<Vec<_>>(),
        [(22., 6.), (22., 3.)]
    );
    assert!(m.paths[0].commands.starts_with(&[
        Move(11., 22.),
        arc(22., 33., 11., true),
        Line(22., 44.)
    ]));
    let join = row_graph(&rows[3], 44.);
    assert!(join.paths[0].commands.starts_with(&[
        Move(22., 0.),
        Line(22., 11.),
        arc(11., 22., 11., true)
    ]));
}
#[test]
fn linear_merge_and_join_paths() {
    let rows = layout(&[c("c", &["b"]), c("b", &["a"])]);
    let g = row_graph(&rows[0], SWIMLANE_HEIGHT);
    assert_eq!(g.width, 22.);
    assert_eq!(paths(&g), [&[Move(11., 11.), Line(11., 22.)]]);
    assert_eq!(
        g.circles,
        [Circle {
            cx: 11.,
            cy: 11.,
            r: 5.,
            stroke_width: 2.,
            fill: Some(COLOR_REGISTRY[0])
        }]
    );
    let rows = merge();
    assert_eq!(
        paths(&row_graph(&rows[0], SWIMLANE_HEIGHT)),
        [
            &[
                Move(11., 11.),
                arc(22., 22., 11., true),
                Move(11., 11.),
                Line(11., 11.)
            ][..],
            &[Move(11., 11.), Line(11., 22.)][..]
        ]
    );
    assert_eq!(
        paths(&row_graph(&rows[3], SWIMLANE_HEIGHT)),
        [
            &[Move(22., 0.), arc(11., 11., 11., true), Line(11., 11.)][..],
            &[Move(11., 0.), Line(11., 11.)][..]
        ]
    );
}
#[test]
fn shifted_lane_draws_elbows_and_head_has_cutout() {
    let rows = layout(&[
        c("M", &["A", "F"]),
        c("C", &["X"]),
        c("A", &["B"]),
        c("F", &["B"]),
        c("B", &["R"]),
        c("X", &["R"]),
        c("R", &[]),
    ]);
    let ids: Vec<_> = rows[4].input.iter().map(|l| l.id.as_str()).collect();
    assert_eq!(ids, ["B", "B", "X"]);
    assert!(paths(&row_graph(&rows[4], SWIMLANE_HEIGHT)).contains(
        &&[
            Move(33., 0.),
            Line(33., 6.),
            arc(28., 11., 5., true),
            Line(27., 11.),
            arc(22., 16., 5., false),
            Line(22., 22.)
        ][..]
    ));
    let mut head = c("c", &["b"]);
    head.head = true;
    let g = row_graph(&layout(&[head])[0], SWIMLANE_HEIGHT);
    assert_eq!(
        g.circles
            .iter()
            .map(|c| (c.r, c.stroke_width, c.fill))
            .collect::<Vec<_>>(),
        [(7., 2., Some(COLOR_REGISTRY[0])), (2., 4., None)]
    );
}
