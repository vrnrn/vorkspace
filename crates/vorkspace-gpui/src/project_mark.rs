//! A project's identity in lists, menus and pickers.
use gpui::{AnyElement, prelude::*, rgb};
use vorkspace_core::project_appearance::project_color;
use vorkspace_ui::theme::{content, icon};

pub(crate) fn project_icon(project: &str, side: f32) -> AnyElement {
    if project.starts_with(vorkspace_core::remote_path::REMOTE_PATH_PREFIX) {
        return icon("internet", side)
            .text_color(content(0.7))
            .into_any_element();
    }
    icon("folder", side)
        .text_color(rgb(project_color(project)))
        .flex_shrink_0()
        .into_any_element()
}
