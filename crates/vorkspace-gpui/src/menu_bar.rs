//! The menu bar's rebuild hook and last focused workspace. Both are written from
//! the shell and read by `app_menu`, which sits above it.
use gpui::{AnyWindowHandle, App, Global, Menu};
use vorkspace_ui::preferences::Preferences;

#[derive(Default)]
pub(crate) struct WorkspaceTarget(pub Option<AnyWindowHandle>);
impl Global for WorkspaceTarget {}

pub(crate) fn remember(window: &gpui::Window, cx: &mut App) {
    cx.set_global(WorkspaceTarget(Some(window.window_handle())));
}

struct Builder(fn(&Preferences) -> Vec<Menu>);
impl Global for Builder {}

pub(crate) fn install(build: fn(&Preferences) -> Vec<Menu>, cx: &mut App) {
    cx.set_global(Builder(build));
}

pub(crate) fn refresh(preferences: &Preferences, cx: &mut App) {
    if let Some(build) = cx.try_global::<Builder>().map(|builder| builder.0) {
        cx.set_menus(build(preferences));
    }
}
