//! One semantic paragraph over shaped prose and separately laid-out inline chips.
use gpui::{
    A11ySubtreeBuilder, AnyElement, App, Bounds, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, SharedString, TextLayout, Window,
    accesskit::{Action, Node, NodeId, Rect, Role},
};
use std::{ops::Range, rc::Rc};

pub(super) type Links = Rc<[(Range<usize>, String)]>;

pub(super) fn append_link(links: &mut Vec<(Range<usize>, String)>, range: Range<usize>, url: &str) {
    if let Some((previous, _)) = links
        .last_mut()
        .filter(|(previous, target)| previous.end == range.start && target == url)
    {
        previous.end = range.end;
    } else {
        links.push((range, url.into()));
    }
}

pub(super) struct LinkedText {
    id: ElementId,
    text: SharedString,
    links: Links,
    fragments: Vec<(usize, TextLayout)>,
    child: AnyElement,
}

pub(super) struct Prepaint {
    scale: f32,
    actions: Vec<(NodeId, usize)>,
}

impl LinkedText {
    pub(super) fn new(
        id: SharedString,
        text: SharedString,
        links: Links,
        fragments: Vec<(usize, TextLayout)>,
        child: AnyElement,
    ) -> Self {
        Self {
            id: format!("{id}-accessible").into(),
            text,
            links,
            fragments,
            child,
        }
    }
}

impl IntoElement for LinkedText {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for LinkedText {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn a11y_role(&self) -> Option<Role> {
        Some(Role::Paragraph)
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        self.child.prepaint(window, cx);
        Prepaint {
            scale: window.scale_factor(),
            actions: Vec::new(),
        }
    }

    fn a11y_synthetic_children(&mut self, state: &mut Prepaint, builder: &mut A11ySubtreeBuilder) {
        let mut fragment_index = 0;
        let mut append = |range: Range<usize>, target: Option<(usize, &str)>| {
            if range.is_empty() {
                return;
            }
            let text = &self.text[range.clone()];
            let mut node = Node::new(if target.is_some() {
                Role::Link
            } else {
                Role::Label
            });
            if let Some((_, url)) = target {
                node.set_label(text);
                node.set_url(url);
                node.add_action(Action::Click);
            } else {
                node.set_value(text);
            }
            let mut bounds: Option<Bounds<Pixels>> = None;
            // Segments and fragments are ordered. Continue from the last
            // consumed fragment so a long rich paragraph stays a linear walk.
            let mut index = fragment_index;
            while let Some((offset, layout)) = self.fragments.get(index) {
                if *offset >= range.end {
                    break;
                }
                let end = *offset + layout.len();
                if end > range.start {
                    layout.for_each_link_bounds_for_range(
                        range.start.saturating_sub(*offset)..(range.end.min(end) - *offset),
                        |rect| bounds = Some(bounds.map_or(rect, |bounds| bounds.union(&rect))),
                    );
                }
                if end <= range.end {
                    fragment_index = index + 1;
                }
                index += 1;
            }
            if let Some(bounds) = bounds {
                let scale = state.scale as f64;
                node.set_bounds(Rect {
                    x0: bounds.left().to_f64() * scale,
                    y0: bounds.top().to_f64() * scale,
                    x1: bounds.right().to_f64() * scale,
                    y1: bounds.bottom().to_f64() * scale,
                });
            }
            let node_id = builder.synthetic_node_id((target.is_some(), range.start));
            if builder.push_child(node_id, node)
                && let Some((index, _)) = target
            {
                state.actions.push((node_id, index));
            }
        };
        let mut cursor = 0;
        for (index, (range, url)) in self.links.iter().enumerate() {
            append(cursor..range.start, None);
            append(range.clone(), Some((index, url)));
            cursor = range.end;
        }
        append(cursor..self.text.len(), None);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        state: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        for &(id, index) in &state.actions {
            let links = self.links.clone();
            window.on_a11y_action(id, Action::Click, move |_, window, cx| {
                super::open_link(&links[index].1, window, cx);
            });
        }
        self.child.paint(window, cx);
    }
}
