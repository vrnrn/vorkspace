//! A project's identity in lists, menus and pickers.
use gpui::{AnyElement, prelude::*, rgb};
use vorkspace_core::project_appearance::project_color;
use vorkspace_ui::theme::icon;

pub(crate) fn project_icon(project: &str, side: f32) -> AnyElement {
    icon("folder", side)
        .text_color(rgb(project_color(project)))
        .flex_shrink_0()
        .into_any_element()
}
