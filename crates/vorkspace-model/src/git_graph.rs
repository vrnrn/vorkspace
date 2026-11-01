//! Port of src/features/source-control/model/gitGraph.ts, itself derived from
//! Visual Studio Code's `scmHistory.ts` (MIT, Copyright (c) Microsoft
//! Corporation). Previous row `output` lanes become the next row's `input`;
//! curves are SVG arcs, not cubics.
use gpui::{Hsla, PathBuilder, Pixels, Point, Window, point, px};
use std::collections::HashMap;

pub const SWIMLANE_HEIGHT: f32 = 22.;
pub const SWIMLANE_WIDTH: f32 = 11.;
const SWIMLANE_CURVE_RADIUS: f32 = 5.;
const CIRCLE_RADIUS: f32 = 4.;
const CIRCLE_STROKE_WIDTH: f32 = 2.;

/// Lane colors, cycled as new branches appear.
pub const COLOR_REGISTRY: [u32; 5] = [0xFFB000, 0xDC267F, 0x994F00, 0x40B0A6, 0xB66DFF];
/// Current branch.
pub const HISTORY_ITEM_REF_COLOR: u32 = 0x75BEFF;
/// Upstream of the current branch.
pub const HISTORY_ITEM_REMOTE_REF_COLOR: u32 = 0xB180D7;
const FALLBACK: u32 = HISTORY_ITEM_REF_COLOR;

#[derive(Clone, Debug, PartialEq)]
pub struct GraphCommit {
    pub sha: String,
    pub parents: Vec<String>,
    pub head: bool,
    /// `(name, kind)`.
    pub refs: Vec<(String, String)>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct GraphRef {
    pub name: String,
    pub kind: String,
    pub color: Option<u32>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Swimlane {
    pub id: String,
    pub color: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Head,
    Node,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub sha: String,
    pub parent_ids: Vec<String>,
    pub kind: NodeKind,
    pub input: Vec<Swimlane>,
    pub output: Vec<Swimlane>,
    pub refs: Vec<GraphRef>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct GraphPath {
    pub d: String,
    pub color: u32,
    pub stroke_width: f32,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Circle {
    pub cx: f32,
    pub cy: f32,
    pub r: f32,
    pub stroke_width: f32,
    pub fill: Option<u32>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct RowGraph {
    pub width: f32,
    pub height: f32,
    pub paths: Vec<GraphPath>,
    pub circles: Vec<Circle>,
    pub kind: NodeKind,
}

fn ref_id(name: &str, kind: &str) -> String {
    format!("{kind}:{name}")
}
fn current_ref_ids(commits: &[GraphCommit]) -> (Option<String>, Option<String>) {
    let head = commits.iter().find(|c| c.head);
    let local = head.and_then(|h| h.refs.iter().find(|(_, kind)| kind == "local"));
    let remote = local.and_then(|(local, _)| {
        head?.refs.iter().find(|(name, kind)| {
            kind == "remote" && (name == local || name.ends_with(&format!("/{local}")))
        })
    });
    (
        local.map(|(n, k)| ref_id(n, k)),
        remote.map(|(n, k)| ref_id(n, k)),
    )
}
/// Ordered as the TS `Map`: insertion order, colorless entries kept.
fn build_color_map(commits: &[GraphCommit]) -> HashMap<String, Option<u32>> {
    let mut map = HashMap::new();
    let (local, remote) = current_ref_ids(commits);
    if let Some(local) = local {
        map.insert(local, Some(HISTORY_ITEM_REF_COLOR));
        if let Some(remote) = remote {
            map.insert(remote, Some(HISTORY_ITEM_REMOTE_REF_COLOR));
        }
    }
    for commit in commits {
        for (name, kind) in &commit.refs {
            map.entry(ref_id(name, kind)).or_insert(None);
        }
    }
    map
}
fn label_color(refs: &[(String, String)], map: &HashMap<String, Option<u32>>) -> Option<u32> {
    refs.iter()
        .find_map(|(name, kind)| map.get(&ref_id(name, kind)).copied().flatten())
}

/// Assign swimlanes for `git log --topo-order` (newest first).
pub fn layout(commits: &[GraphCommit]) -> Vec<Row> {
    let colors = build_color_map(commits);
    let (current_local, current_remote) = current_ref_ids(commits);
    let by_sha: HashMap<&str, &GraphCommit> = commits.iter().map(|c| (c.sha.as_str(), c)).collect();
    let mut color_index: isize = -1;
    let mut rows: Vec<Row> = Vec::with_capacity(commits.len());
    for commit in commits {
        let kind = if commit.head {
            NodeKind::Head
        } else {
            NodeKind::Node
        };
        let input = rows.last().map(|r| r.output.clone()).unwrap_or_default();
        let mut output = Vec::new();
        let parent_ids: Vec<String> = commit
            .parents
            .iter()
            .filter(|p| !p.is_empty())
            .cloned()
            .collect();
        let mut first_parent_added = false;
        if !parent_ids.is_empty() {
            for node in &input {
                if node.id == commit.sha {
                    if !first_parent_added {
                        output.push(Swimlane {
                            id: parent_ids[0].clone(),
                            color: label_color(&commit.refs, &colors).unwrap_or(node.color),
                        });
                        first_parent_added = true;
                    }
                    continue;
                }
                output.push(node.clone());
            }
        }
        for (i, parent) in parent_ids
            .iter()
            .enumerate()
            .skip(usize::from(first_parent_added))
        {
            let color = if i == 0 {
                label_color(&commit.refs, &colors)
            } else {
                by_sha
                    .get(parent.as_str())
                    .and_then(|p| label_color(&p.refs, &colors))
            };
            let color = color.unwrap_or_else(|| {
                color_index = (color_index + 1).rem_euclid(COLOR_REGISTRY.len() as isize);
                COLOR_REGISTRY[color_index as usize]
            });
            output.push(Swimlane {
                id: parent.clone(),
                color,
            });
        }
        let circle_index = input
            .iter()
            .position(|n| n.id == commit.sha)
            .unwrap_or(input.len());
        let mut refs: Vec<GraphRef> = commit
            .refs
            .iter()
            .map(|(name, kind)| {
                let id = ref_id(name, kind);
                let color = match colors.get(&id) {
                    Some(Some(color)) => Some(*color),
                    Some(None) => Some(
                        output
                            .get(circle_index)
                            .or_else(|| input.get(circle_index))
                            .map_or(FALLBACK, |lane| lane.color),
                    ),
                    None => None,
                };
                GraphRef {
                    name: name.clone(),
                    kind: kind.clone(),
                    color,
                }
            })
            .collect();
        let order = |r: &GraphRef| {
            let id = ref_id(&r.name, &r.kind);
            if current_local.as_ref() == Some(&id) {
                1
            } else if current_remote.as_ref() == Some(&id) {
                2
            } else if r.color.is_some() {
                4
            } else {
                99
            }
        };
        refs.sort_by_key(order);
        rows.push(Row {
            sha: commit.sha.clone(),
            parent_ids,
            kind,
            input,
            output,
            refs,
        });
    }
    rows
}

/// One row of graph paths and circles. Taller rows extend lanes above and
/// below the node while keeping the circles and curve radii at their size.
pub fn row_graph(row: &Row, row_height: f32) -> RowGraph {
    const W: f32 = SWIMLANE_WIDTH;
    const R: f32 = SWIMLANE_CURVE_RADIUS;
    let center = row_height / 2.;
    let pad = (row_height - SWIMLANE_HEIGHT) / 2.;
    let (input, output) = (&row.input, &row.output);
    let input_index = input.iter().position(|n| n.id == row.sha);
    let circle_index = input_index.unwrap_or(input.len());
    let circle_color = output
        .get(circle_index)
        .or_else(|| input.get(circle_index))
        .map_or(FALLBACK, |lane| lane.color);
    let mut paths = Vec::new();
    let mut push = |d: String, color: u32| {
        paths.push(GraphPath {
            d,
            color,
            stroke_width: 1.,
        })
    };
    let n = |v: f32| fmt(v);
    let mut out_ix = 0;
    for (index, lane) in input.iter().enumerate() {
        let i = index as f32;
        if lane.id == row.sha {
            if index != circle_index {
                let mut d = vec![format!("M {} 0", n(W * (i + 1.)))];
                if pad > 0. {
                    d.push(format!("V {}", n(pad)));
                }
                d.push(format!("A {W} {W} 0 0 1 {} {}", n(W * i), n(center)));
                d.push(format!("H {}", n(W * (circle_index as f32 + 1.))));
                push(d.join(" "), lane.color);
            } else {
                out_ix += 1;
            }
        } else if out_ix < output.len() && lane.id == output[out_ix].id {
            let o = out_ix as f32;
            if index == out_ix {
                push(
                    format!("M {} 0 V {}", n(W * (i + 1.)), n(row_height)),
                    lane.color,
                );
            } else {
                push(
                    [
                        format!("M {} 0", n(W * (i + 1.))),
                        format!("V {}", n(center - R)),
                        format!("A {R} {R} 0 0 1 {} {}", n(W * (i + 1.) - R), n(center)),
                        format!("H {}", n(W * (o + 1.) + R)),
                        format!("A {R} {R} 0 0 0 {} {}", n(W * (o + 1.)), n(center + R)),
                        format!("V {}", n(row_height)),
                    ]
                    .join(" "),
                    lane.color,
                );
            }
            out_ix += 1;
        }
    }
    for parent in row.parent_ids.iter().skip(1) {
        let Some(ix) = output.iter().rposition(|lane| &lane.id == parent) else {
            continue;
        };
        let p = ix as f32;
        let mut d = vec![
            format!("M {} {}", n(W * p), n(center)),
            format!("A {W} {W} 0 0 1 {} {}", n(W * (p + 1.)), n(center + W)),
        ];
        if pad > 0. {
            d.push(format!("V {}", n(row_height)));
        }
        d.push(format!("M {} {}", n(W * p), n(center)));
        d.push(format!("H {} ", n(W * (circle_index as f32 + 1.))));
        push(d.join(" "), output[ix].color);
    }
    let cx = W * (circle_index as f32 + 1.);
    if let Some(ix) = input_index {
        push(format!("M {} 0 V {}", n(cx), n(center)), input[ix].color);
    }
    if !row.parent_ids.is_empty() {
        push(
            format!("M {} {} V {}", n(cx), n(center), n(row_height)),
            circle_color,
        );
    }
    let circle = |r: f32, stroke_width: f32, fill: Option<u32>| Circle {
        cx,
        cy: center,
        r,
        stroke_width,
        fill,
    };
    let circles = if row.kind == NodeKind::Head {
        vec![
            circle(CIRCLE_RADIUS + 3., CIRCLE_STROKE_WIDTH, Some(circle_color)),
            circle(CIRCLE_STROKE_WIDTH, CIRCLE_RADIUS, None),
        ]
    } else if row.parent_ids.len() > 1 {
        vec![
            circle(CIRCLE_RADIUS + 2., CIRCLE_STROKE_WIDTH, Some(circle_color)),
            circle(CIRCLE_RADIUS - 1., CIRCLE_STROKE_WIDTH, Some(circle_color)),
        ]
    } else {
        vec![circle(
            CIRCLE_RADIUS + 1.,
            CIRCLE_STROKE_WIDTH,
            Some(circle_color),
        )]
    };
    RowGraph {
        width: W * (input.len().max(output.len()).max(1) as f32 + 1.),
        height: row_height,
        paths,
        circles,
        kind: row.kind,
    }
}
/// JavaScript's number formatting for the coordinates used here.
fn fmt(v: f32) -> String {
    if v.fract() == 0. {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// Strokes the `M`/`V`/`H`/`A` subset `row_graph` emits, relative to `origin`.
pub fn stroke_path(d: &str, origin: Point<Pixels>, width: f32, color: Hsla, window: &mut Window) {
    let mut builder = PathBuilder::stroke(px(width));
    let mut tokens = d.split_whitespace();
    let mut at = point(0f32, 0f32);
    let to = |p: Point<f32>| origin + point(px(p.x), px(p.y));
    let num = |t: &mut std::str::SplitWhitespace| -> f32 {
        t.next().and_then(|v| v.parse().ok()).unwrap_or(0.)
    };
    while let Some(command) = tokens.next() {
        match command {
            "M" => {
                at = point(num(&mut tokens), num(&mut tokens));
                builder.move_to(to(at));
            }
            "V" => {
                at.y = num(&mut tokens);
                builder.line_to(to(at));
            }
            "H" => {
                at.x = num(&mut tokens);
                builder.line_to(to(at));
            }
            "A" => {
                let (rx, ry) = (num(&mut tokens), num(&mut tokens));
                let rotation = num(&mut tokens);
                let large = num(&mut tokens) != 0.;
                let sweep = num(&mut tokens) != 0.;
                at = point(num(&mut tokens), num(&mut tokens));
                builder.arc_to(point(px(rx), px(ry)), px(rotation), large, sweep, to(at));
            }
            _ => {}
        }
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

#[cfg(test)]
mod tests;
