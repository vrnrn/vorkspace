//! Choose the toolbar from its actual layout in the same frame as a resize.
use gpui::{
    AnyElement, App, AvailableSpace, Bounds, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, Style, Window, px, size,
};

pub(super) struct ResponsiveToolbar {
    wide: AnyElement,
    compact: AnyElement,
}
impl ResponsiveToolbar {
    pub(super) fn new(wide: AnyElement, compact: AnyElement) -> Self {
        Self { wide, compact }
    }
}
impl IntoElement for ResponsiveToolbar {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for ResponsiveToolbar {
    type RequestLayoutState = ();
    type PrepaintState = bool;
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = gpui::relative(1.).into();
        // Both presentations occupy a single 26pt row plus 4pt top inset.
        style.size.height = px(30.).into();
        (window.request_layout(style, [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let available = size(
            AvailableSpace::Definite(bounds.size.width),
            AvailableSpace::MaxContent,
        );
        let wide_size = self.wide.layout_as_root(available, window, cx);
        let compact = wide_size.height > px(30.5);
        if compact {
            self.compact.layout_as_root(available, window, cx);
            self.compact.prepaint_at(bounds.origin, window, cx);
        } else {
            self.wide.prepaint_at(bounds.origin, window, cx);
        }
        compact
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        compact: &mut bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        if *compact {
            self.compact.paint(window, cx);
        } else {
            self.wide.paint(window, cx);
        }
    }
}
