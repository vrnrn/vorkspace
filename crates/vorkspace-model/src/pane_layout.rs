//! Port of src/features/workspace/model/layout.ts: a tab's split tree.
//! Same-direction splits share a group so new panes divide space equally
//! until a sash moves. A pane dropped on another pane's edge reorders
//! same-axis siblings, nests a perpendicular split, or relocates into the
//! target's group. Splits are addressed by their child-index path.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitDir {
    Right,
    Down,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Before,
    After,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PaneNode {
    Leaf {
        id: i64,
    },
    Split {
        dir: SplitDir,
        children: Vec<PaneNode>,
        sizes: Vec<f32>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}
impl Rect {
    pub const FULL: Rect = Rect {
        x: 0.,
        y: 0.,
        w: 1.,
        h: 1.,
    };
    pub fn lerp(self, to: Rect, t: f32) -> Rect {
        Rect {
            x: self.x + (to.x - self.x) * t,
            y: self.y + (to.y - self.y) * t,
            w: self.w + (to.w - self.w) * t,
            h: self.h + (to.h - self.h) * t,
        }
    }
    fn child(self, dir: SplitDir, offset: f32, size: f32) -> Rect {
        match dir {
            SplitDir::Right => Rect {
                x: self.x + offset * self.w,
                w: size * self.w,
                ..self
            },
            SplitDir::Down => Rect {
                y: self.y + offset * self.h,
                h: size * self.h,
                ..self
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutLeaf {
    pub id: i64,
    pub rect: Rect,
    /// The axis its parent split runs along.
    pub axis: SplitDir,
}
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutSash {
    pub path: Vec<usize>,
    pub index: usize,
    pub dir: SplitDir,
    pub group: Rect,
    pub boundary: f32,
}

const MIN_SIZE: f32 = 0.08;

pub fn leaf(id: i64) -> PaneNode {
    PaneNode::Leaf { id }
}
fn split(dir: SplitDir, children: Vec<PaneNode>) -> PaneNode {
    let sizes = equal_sizes(children.len());
    PaneNode::Split {
        dir,
        children,
        sizes,
    }
}
fn equal_sizes(n: usize) -> Vec<f32> {
    vec![1. / n as f32; n]
}
fn normalize(sizes: &[f32]) -> Vec<f32> {
    let total: f32 = sizes.iter().sum();
    if total <= 0. {
        return equal_sizes(sizes.len());
    }
    sizes.iter().map(|s| s / total).collect()
}
fn edge_split(edge: Edge) -> (SplitDir, Place) {
    match edge {
        Edge::Left => (SplitDir::Right, Place::Before),
        Edge::Right => (SplitDir::Right, Place::After),
        Edge::Top => (SplitDir::Down, Place::Before),
        Edge::Bottom => (SplitDir::Down, Place::After),
    }
}

/// The edge nearest the point, from the pane's centre.
pub fn edge_from_point(x: f32, y: f32, rect: Rect) -> Edge {
    let nx = if rect.w <= 0. {
        0.
    } else {
        (x - rect.x) / rect.w - 0.5
    };
    let ny = if rect.h <= 0. {
        0.
    } else {
        (y - rect.y) / rect.h - 0.5
    };
    if nx.abs() > ny.abs() {
        if nx < 0. { Edge::Left } else { Edge::Right }
    } else if ny < 0. {
        Edge::Top
    } else {
        Edge::Bottom
    }
}

impl PaneNode {
    /// Largest number of session leaves intersected by any vertical line.
    /// File panes are excluded by the caller, but still occupy layout space.
    pub fn max_session_stack(&self, is_session: impl Fn(i64) -> bool) -> usize {
        let mut edges: Vec<_> = self
            .leaves()
            .into_iter()
            .filter(|p| is_session(p.id) && p.rect.w > 0.)
            .flat_map(|p| [(p.rect.x, 1isize), (p.rect.x + p.rect.w, -1)])
            .collect();
        // At a shared boundary, close the left interval before opening the
        // right one; touching columns do not form an extra vertical stack.
        edges.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut current = 0isize;
        let mut max = 0isize;
        let mut index = 0;
        while index < edges.len() {
            let x = edges[index].0;
            let mut change = 0;
            while index < edges.len() && edges[index].0 - x < 0.000_001 {
                change += edges[index].1;
                index += 1;
            }
            current += change;
            max = max.max(current);
        }
        max as usize
    }

    /// The width required to render each leaf at least `minimum` wide while
    /// retaining the persisted split ratios and all leaf identities.
    pub fn width_for_minimum(&self, minimum: f32) -> f32 {
        self.leaves()
            .iter()
            .map(|p| minimum / p.rect.w.max(0.000_001))
            .fold(0., f32::max)
    }

    /// Minimum width of the tree when horizontal shares may be adjusted.
    pub fn minimum_width(&self, minimum: f32) -> f32 {
        match self {
            PaneNode::Leaf { .. } => minimum,
            PaneNode::Split {
                dir: SplitDir::Right,
                children,
                ..
            } => children.iter().map(|c| c.minimum_width(minimum)).sum(),
            PaneNode::Split {
                dir: SplitDir::Down,
                children,
                ..
            } => children
                .iter()
                .map(|c| c.minimum_width(minimum))
                .fold(0., f32::max),
        }
    }

    /// Preserve existing shares unless a child needs more room. Only the
    /// horizontal axis changes; vertical split sizes and leaf order stay put.
    pub fn fit_width(&mut self, width: f32, minimum: f32) {
        let PaneNode::Split {
            dir,
            children,
            sizes,
        } = self
        else {
            return;
        };
        if *dir == SplitDir::Down {
            for child in children {
                child.fit_width(width, minimum);
            }
            return;
        }
        let required: Vec<_> = children.iter().map(|c| c.minimum_width(minimum)).collect();
        if width <= 0. || required.iter().sum::<f32>() > width + 0.01 {
            return;
        }
        let weights = normalize(sizes);
        let mut allocated = vec![0.; children.len()];
        let mut remaining = width;
        let mut open: Vec<usize> = (0..children.len()).collect();
        loop {
            let total: f32 = open.iter().map(|i| weights[*i].max(0.)).sum();
            let under: Vec<_> = open
                .iter()
                .copied()
                .filter(|i| {
                    let share = if total > 0. {
                        weights[*i].max(0.) / total
                    } else {
                        1. / open.len() as f32
                    };
                    remaining * share < required[*i]
                })
                .collect();
            if under.is_empty() {
                for i in &open {
                    let share = if total > 0. {
                        weights[*i].max(0.) / total
                    } else {
                        1. / open.len() as f32
                    };
                    allocated[*i] = remaining * share;
                }
                break;
            }
            for i in under {
                allocated[i] = required[i];
                remaining -= required[i];
                open.retain(|v| *v != i);
            }
            if open.is_empty() {
                break;
            }
        }
        for (child, child_width) in children.iter_mut().zip(&allocated) {
            child.fit_width(*child_width, minimum);
        }
        *sizes = allocated.iter().map(|w| *w / width).collect();
    }

    pub fn height_for_minimum(&self, minimum: f32, is_session: impl Fn(i64) -> bool) -> f32 {
        self.leaves()
            .iter()
            .filter(|p| is_session(p.id))
            .map(|p| minimum / p.rect.h.max(0.000_001))
            .fold(0., f32::max)
    }
    pub fn leaf_ids(&self) -> Vec<i64> {
        match self {
            PaneNode::Leaf { id } => vec![*id],
            PaneNode::Split { children, .. } => children.iter().flat_map(Self::leaf_ids).collect(),
        }
    }
    pub fn contains(&self, id: i64) -> bool {
        match self {
            PaneNode::Leaf { id: leaf } => *leaf == id,
            PaneNode::Split { children, .. } => children.iter().any(|c| c.contains(id)),
        }
    }
    pub fn first_leaf(&self) -> i64 {
        match self {
            PaneNode::Leaf { id } => *id,
            PaneNode::Split { children, .. } => children[0].first_leaf(),
        }
    }
    pub fn is_split(&self) -> bool {
        matches!(self, PaneNode::Split { .. })
    }

    /// cmd-d / shift-cmd-d: a new pane after the focused one.
    pub fn split_pane(&self, focused: i64, dir: SplitDir, new: i64) -> PaneNode {
        match self {
            PaneNode::Leaf { id } if *id == focused => split(dir, vec![self.clone(), leaf(new)]),
            PaneNode::Leaf { .. } => self.clone(),
            PaneNode::Split {
                dir: own,
                children,
                sizes,
            } => {
                if let Some(ix) = children
                    .iter()
                    .position(|c| matches!(c, PaneNode::Leaf { id } if *id == focused))
                {
                    let mut children = children.clone();
                    if *own == dir {
                        children.insert(ix + 1, leaf(new));
                        return split(dir, children);
                    }
                    children[ix] = split(dir, vec![children[ix].clone(), leaf(new)]);
                    return PaneNode::Split {
                        dir: *own,
                        children,
                        sizes: sizes.clone(),
                    };
                }
                PaneNode::Split {
                    dir: *own,
                    children: children
                        .iter()
                        .map(|c| c.split_pane(focused, dir, new))
                        .collect(),
                    sizes: sizes.clone(),
                }
            }
        }
    }

    /// Swap one leaf for another, keeping the tree intact.
    pub fn replace_leaf(&self, from: i64, to: i64) -> PaneNode {
        match self {
            PaneNode::Leaf { id } if *id == from => leaf(to),
            PaneNode::Leaf { .. } => self.clone(),
            PaneNode::Split {
                dir,
                children,
                sizes,
            } => PaneNode::Split {
                dir: *dir,
                children: children.iter().map(|c| c.replace_leaf(from, to)).collect(),
                sizes: sizes.clone(),
            },
        }
    }

    /// Drop a leaf; a parent left with one child collapses to it.
    pub fn remove(&self, target: i64) -> Option<PaneNode> {
        match self {
            PaneNode::Leaf { id } => (*id != target).then(|| self.clone()),
            PaneNode::Split {
                dir,
                children,
                sizes,
            } => {
                let kept: Vec<_> = children
                    .iter()
                    .zip(sizes)
                    .filter_map(|(c, s)| c.remove(target).map(|c| (c, *s)))
                    .collect();
                match kept.len() {
                    0 => None,
                    1 => kept.into_iter().next().map(|(c, _)| c),
                    _ => {
                        let sizes: Vec<_> = kept.iter().map(|(_, s)| *s).collect();
                        Some(PaneNode::Split {
                            dir: *dir,
                            children: kept.into_iter().map(|(c, _)| c).collect(),
                            sizes: normalize(&sizes),
                        })
                    }
                }
            }
        }
    }

    /// The pane to focus after closing `id`: a neighbour's first leaf.
    pub fn sibling(&self, id: i64) -> Option<i64> {
        let PaneNode::Split { children, .. } = self else {
            return None;
        };
        if let Some(ix) = children
            .iter()
            .position(|c| matches!(c, PaneNode::Leaf { id: l } if *l == id))
        {
            let neighbour = if ix > 0 {
                children.get(ix - 1)
            } else {
                children.get(ix + 1)
            };
            return neighbour.map(Self::first_leaf);
        }
        children.iter().find_map(|c| c.sibling(id))
    }

    /// Move the sash between `index` and `index + 1` of the split at `path`
    /// to `boundary`, a 0–1 position within its group. It may pass the
    /// `sash_range` a little, so a drag can stretch beyond it and spring back.
    pub fn place_sash(&mut self, path: &[usize], index: usize, boundary: f32) {
        if let Some(sizes) = self.sizes_mut(path) {
            *sizes = sizes_at_boundary(sizes, index, boundary, MIN_SIZE / 4.);
        }
    }

    /// The sash's current boundary and the range it rests in.
    pub fn sash_range(&self, path: &[usize], index: usize) -> Option<(f32, f32, f32)> {
        let PaneNode::Split {
            children, sizes, ..
        } = self
        else {
            return None;
        };
        if let Some((first, rest)) = path.split_first() {
            return children.get(*first)?.sash_range(rest, index);
        }
        if index + 1 >= sizes.len() {
            return None;
        }
        let before: f32 = sizes[..index].iter().sum();
        let pair = sizes[index] + sizes[index + 1];
        let min = MIN_SIZE.min(pair / 2.);
        Some((before + sizes[index], before + min, before + pair - min))
    }

    pub fn horizontal_sash_range(
        &self,
        path: &[usize],
        index: usize,
        span: f32,
        minimum: f32,
    ) -> Option<(f32, f32, f32)> {
        let PaneNode::Split {
            dir,
            children,
            sizes,
        } = self
        else {
            return None;
        };
        if let Some((first, rest)) = path.split_first() {
            return children
                .get(*first)?
                .horizontal_sash_range(rest, index, span, minimum);
        }
        if *dir != SplitDir::Right || index + 1 >= sizes.len() || span <= 0. {
            return None;
        }
        let before: f32 = sizes[..index].iter().sum();
        let pair = sizes[index] + sizes[index + 1];
        let left = children[index].width_for_minimum(minimum) / span;
        let right = children[index + 1].width_for_minimum(minimum) / span;
        if left + right > pair {
            return None;
        }
        Some((
            before + sizes[index],
            before + left.min(pair),
            before + pair - right.min(pair),
        ))
    }

    fn sizes_mut(&mut self, path: &[usize]) -> Option<&mut Vec<f32>> {
        let PaneNode::Split {
            children, sizes, ..
        } = self
        else {
            return None;
        };
        match path.split_first() {
            Some((first, rest)) => children.get_mut(*first)?.sizes_mut(rest),
            None => Some(sizes),
        }
    }

    pub fn leaves(&self) -> Vec<LayoutLeaf> {
        let mut out = Vec::new();
        self.collect_leaves(Rect::FULL, SplitDir::Right, &mut out);
        out
    }
    fn collect_leaves(&self, rect: Rect, axis: SplitDir, out: &mut Vec<LayoutLeaf>) {
        match self {
            PaneNode::Leaf { id } => out.push(LayoutLeaf {
                id: *id,
                rect,
                axis,
            }),
            PaneNode::Split {
                dir,
                children,
                sizes,
            } => {
                let mut offset = 0.;
                for (child, size) in children.iter().zip(sizes) {
                    child.collect_leaves(rect.child(*dir, offset, *size), *dir, out);
                    offset += size;
                }
            }
        }
    }
    pub fn sashes(&self) -> Vec<LayoutSash> {
        let mut out = Vec::new();
        self.collect_sashes(Rect::FULL, &mut Vec::new(), &mut out);
        out
    }
    fn collect_sashes(&self, rect: Rect, path: &mut Vec<usize>, out: &mut Vec<LayoutSash>) {
        let PaneNode::Split {
            dir,
            children,
            sizes,
        } = self
        else {
            return;
        };
        let mut offset = 0.;
        for (ix, (child, size)) in children.iter().zip(sizes).enumerate() {
            if ix > 0 {
                out.push(LayoutSash {
                    path: path.clone(),
                    index: ix - 1,
                    dir: *dir,
                    group: rect,
                    boundary: offset,
                });
            }
            path.push(ix);
            child.collect_sashes(rect.child(*dir, offset, *size), path, out);
            path.pop();
            offset += size;
        }
    }

    /// cmd-opt-arrows: the adjacent pane, preferring ones sharing an edge.
    pub fn neighbor(&self, focused: i64, dir: Edge) -> Option<i64> {
        let panes = self.leaves();
        let c = panes.iter().find(|p| p.id == focused)?.rect;
        let overlap = |a0: f32, a1: f32, b0: f32, b1: f32| (a1.min(b1) - a0.max(b0)).max(0.);
        let mut best: Option<(i64, u8, f32, f32)> = None;
        for pane in &panes {
            if pane.id == focused {
                continue;
            }
            let r = pane.rect;
            let (gap, perp) = match dir {
                Edge::Left if r.x + r.w <= c.x + 1e-4 => {
                    (c.x - (r.x + r.w), overlap(c.y, c.y + c.h, r.y, r.y + r.h))
                }
                Edge::Right if r.x >= c.x + c.w - 1e-4 => {
                    (r.x - (c.x + c.w), overlap(c.y, c.y + c.h, r.y, r.y + r.h))
                }
                Edge::Top if r.y + r.h <= c.y + 1e-4 => {
                    (c.y - (r.y + r.h), overlap(c.x, c.x + c.w, r.x, r.x + r.w))
                }
                Edge::Bottom if r.y >= c.y + c.h - 1e-4 => {
                    (r.y - (c.y + c.h), overlap(c.x, c.x + c.w, r.x, r.x + r.w))
                }
                _ => continue,
            };
            let hit = if perp > 0. { 0 } else { 1 };
            let better = best.is_none_or(|(_, bh, bg, bo)| {
                hit < bh || (hit == bh && gap < bg) || (hit == bh && gap == bg && perp > bo)
            });
            if better {
                best = Some((pane.id, hit, gap, perp));
            }
        }
        best.map(|b| b.0)
    }

    fn leaf_parent(&self, target: i64) -> Option<(Vec<usize>, usize, SplitDir)> {
        fn walk(
            node: &PaneNode,
            target: i64,
            path: &mut Vec<usize>,
        ) -> Option<(Vec<usize>, usize, SplitDir)> {
            let PaneNode::Split { dir, children, .. } = node else {
                return None;
            };
            for (ix, child) in children.iter().enumerate() {
                if matches!(child, PaneNode::Leaf { id } if *id == target) {
                    return Some((path.clone(), ix, *dir));
                }
                path.push(ix);
                let found = walk(child, target, path);
                path.pop();
                if found.is_some() {
                    return found;
                }
            }
            None
        }
        walk(self, target, &mut Vec::new())
    }
    fn at_path(&mut self, path: &[usize]) -> &mut PaneNode {
        match path.split_first() {
            Some((first, rest)) => match self {
                PaneNode::Split { children, .. } => children[*first].at_path(rest),
                PaneNode::Leaf { .. } => self,
            },
            None => self,
        }
    }
    fn insert_beside(&self, target: i64, incoming: PaneNode, place: Place) -> PaneNode {
        let PaneNode::Split {
            dir,
            children,
            sizes,
        } = self
        else {
            return self.clone();
        };
        if let Some(ix) = children
            .iter()
            .position(|c| matches!(c, PaneNode::Leaf { id } if *id == target))
        {
            let mut children = children.clone();
            let mut sizes = sizes.clone();
            let share = sizes[ix] / 2.;
            sizes[ix] = share;
            let at = if place == Place::Before { ix } else { ix + 1 };
            children.insert(at, incoming);
            sizes.insert(at, share);
            return PaneNode::Split {
                dir: *dir,
                children,
                sizes,
            };
        }
        PaneNode::Split {
            dir: *dir,
            children: children
                .iter()
                .map(|c| c.insert_beside(target, incoming.clone(), place))
                .collect(),
            sizes: sizes.clone(),
        }
    }
    fn wrap_beside(
        &self,
        target: i64,
        incoming: PaneNode,
        dir: SplitDir,
        place: Place,
    ) -> PaneNode {
        match self {
            PaneNode::Leaf { id } if *id == target => {
                let children = if place == Place::Before {
                    vec![incoming, self.clone()]
                } else {
                    vec![self.clone(), incoming]
                };
                split(dir, children)
            }
            PaneNode::Leaf { .. } => self.clone(),
            PaneNode::Split {
                dir: own,
                children,
                sizes,
            } => PaneNode::Split {
                dir: *own,
                children: children
                    .iter()
                    .map(|c| c.wrap_beside(target, incoming.clone(), dir, place))
                    .collect(),
                sizes: sizes.clone(),
            },
        }
    }

    /// Drag a leaf onto another pane's edge. Same-axis siblings swap order
    /// and keep their sizes; otherwise the leaf moves beside the target.
    pub fn move_pane(&self, from: i64, to: i64, edge: Edge) -> PaneNode {
        if from == to {
            return self.clone();
        }
        let (Some(from_at), Some(to_at)) = (self.leaf_parent(from), self.leaf_parent(to)) else {
            return self.clone();
        };
        let (dir, place) = edge_split(edge);
        if to_at.2 == dir && from_at.0 == to_at.0 {
            let mut next = self.clone();
            if let PaneNode::Split {
                children, sizes, ..
            } = next.at_path(&from_at.0)
            {
                let (from_ix, n) = (from_at.1, children.len());
                let mut at = if place == Place::After {
                    to_at.1 + 1
                } else {
                    to_at.1
                }
                .min(n);
                if from_ix < at {
                    at -= 1;
                }
                if from_ix != at {
                    let child = children.remove(from_ix);
                    let size = sizes.remove(from_ix);
                    children.insert(at, child);
                    sizes.insert(at, size);
                }
            }
            return next;
        }
        let Some(tree) = self.remove(from) else {
            return self.clone();
        };
        if !tree.contains(to) {
            return self.clone();
        }
        tree.place_layout(leaf(from), to, edge)
    }

    /// Open `id` on `to`'s edge, or move it there when already present.
    pub fn place_pane(&self, id: i64, to: i64, edge: Edge) -> PaneNode {
        if id == to || !self.contains(to) {
            return self.clone();
        }
        if self.contains(id) {
            return self.move_pane(id, to, edge);
        }
        self.place_layout(leaf(id), to, edge)
    }

    /// Place an intact tree beside one pane.
    pub fn place_layout(&self, incoming: PaneNode, to: i64, edge: Edge) -> PaneNode {
        if !self.contains(to) {
            return self.clone();
        }
        let (dir, place) = edge_split(edge);
        match self.leaf_parent(to) {
            Some((_, _, parent)) if parent == dir => self.insert_beside(to, incoming, place),
            _ => self.wrap_beside(to, incoming, dir, place),
        }
    }

    /// Drop unknown or repeated leaves, as restore does. None when empty.
    pub fn retain(&self, keep: &mut impl FnMut(i64) -> bool) -> Option<PaneNode> {
        match self {
            PaneNode::Leaf { id } => keep(*id).then(|| self.clone()),
            PaneNode::Split {
                dir,
                children,
                sizes,
            } => {
                if children.len() != sizes.len() {
                    let ids = self.leaf_ids();
                    let kept: Vec<_> = ids.into_iter().filter(|id| keep(*id)).map(leaf).collect();
                    return match kept.len() {
                        0 => None,
                        1 => kept.into_iter().next(),
                        _ => Some(split(*dir, kept)),
                    };
                }
                let kept: Vec<_> = children
                    .iter()
                    .zip(sizes)
                    .filter_map(|(c, s)| c.retain(keep).map(|c| (c, *s)))
                    .collect();
                match kept.len() {
                    0 => None,
                    1 => kept.into_iter().next().map(|(c, _)| c),
                    _ => {
                        let sizes: Vec<_> = kept.iter().map(|(_, s)| *s).collect();
                        Some(PaneNode::Split {
                            dir: *dir,
                            children: kept.into_iter().map(|(c, _)| c).collect(),
                            sizes: normalize(&sizes),
                        })
                    }
                }
            }
        }
    }
}

pub fn sizes_at_boundary(current: &[f32], index: usize, boundary: f32, min_size: f32) -> Vec<f32> {
    if index + 1 >= current.len() {
        return current.to_vec();
    }
    let mut sizes = current.to_vec();
    let before: f32 = sizes[..index].iter().sum();
    let pair = sizes[index] + sizes[index + 1];
    let min = min_size.min(pair / 2.);
    let first = (boundary - before).max(min).min(pair - min);
    sizes[index] = first;
    sizes[index + 1] = pair - first;
    sizes
}

#[cfg(test)]
mod tests;
