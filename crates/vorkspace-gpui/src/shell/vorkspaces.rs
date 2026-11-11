//! Vorkspace selection, project routing and the rail switcher.
use super::*;
use crate::popover::{MenuAction, MenuItem};
use gpui::Div;
use unicode_segmentation::UnicodeSegmentation;
use vorkspace_ui::radius;

/// A stable tint for a vorkspace's initial, from its id.
fn vorkspace_hue(id: &str) -> f32 {
    let hash = id
        .bytes()
        .fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    (hash % 360) as f32 / 360.
}

pub(crate) fn vorkspace_mark(def: &crate::workspace::VorkspaceDef, side: f32) -> Div {
    let label = def.icon_text();
    let multiple = label.graphemes(true).nth(1).is_some();
    let background: gpui::Hsla = match def.color {
        Some(color) => gpui::rgb(color).into(),
        None => gpui::hsla(vorkspace_hue(&def.id), 0.55, 0.5, 1.),
    };
    let rgb: gpui::Rgba = background.into();
    let linear = |channel: f32| {
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * linear(rgb.r) + 0.7152 * linear(rgb.g) + 0.0722 * linear(rgb.b);
    div()
        .size(px(side))
        .flex_shrink_0()
        .rounded(px(vorkspace_ui::radius::CONTROL))
        .bg(background)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(side * if multiple { 0.42 } else { 0.55 }))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(if luminance > 0.179 {
            gpui::black()
        } else {
            gpui::white()
        })
        .child(label)
}

impl Shell {
    pub(super) fn switch_vorkspace(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let landing = self.workspace.vorkspace_landing_where(id, |path| {
            !self.project_is_deleting(path, cx)
                && !vorkspace_core::worktree_management::is_being_removed(path)
        });
        if landing.is_empty() {
            let preferred = self.workspace.vorkspace_landing(id);
            if !preferred.is_empty() && !self.allow_project_entry(&preferred, cx) {
                return;
            }
        }
        if !self.select_vorkspace(id, cx) {
            return;
        }
        self.open_project_from_user(landing, window, cx);
        self.persist(cx);
        cx.notify();
    }

    fn select_vorkspace(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        if id == self.workspace.active_vorkspace
            || !self.workspace.vorkspaces.iter().any(|v| v.id == id)
        {
            return false;
        }
        let from = self.workspace.active_vorkspace.clone();
        if self
            .workspace
            .vorkspace_has_project(&self.workspace.project)
        {
            self.workspace
                .vorkspace_project
                .insert(from, self.workspace.project.clone());
        }
        self.workspace.active_vorkspace = id.to_owned();
        self.rail_sessions_expanded = false;
        self.rail_search_open = false;
        self.query.clear();
        self.sidebar_search
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        self.selected_sessions.clear();
        self.selection_anchor = None;
        self.sidebar_target = None;
        self.configure_vorkspace_bots(cx);
        true
    }

    /// Cross-project history and search keep the current vorkspace when it
    /// shares the folder, otherwise they take the user to a vorkspace with it.
    pub(super) fn route_project(&mut self, path: &str, cx: &mut Context<Self>) -> bool {
        if path.is_empty() {
            return true;
        }
        if !self.allow_project_entry(path, cx) {
            return false;
        }
        let Some(id) = self
            .workspace
            .vorkspace_for_project(path)
            .map(str::to_owned)
        else {
            self.error = Some(if self.workspace.is_project_available(path) {
                "Add this project to a vorkspace before opening its conversations.".into()
            } else {
                "Restore this project before opening its conversations.".into()
            });
            cx.notify();
            return false;
        };
        if self.select_vorkspace(&id, cx) {
            self.workspace.project = path.to_owned();
            self.workspace.active = None;
        }
        true
    }

    pub(super) fn vorkspace_menu_items(&self) -> (SharedString, Vec<MenuItem>, f32, MenuStyle) {
        let mut items: Vec<MenuItem> = self
            .workspace
            .vorkspaces
            .iter()
            .map(|v| {
                MenuAction::new(format!("switch:{}", v.id), v.name.clone())
                    .checked(v.id == self.workspace.active_vorkspace)
                    .into()
            })
            .collect();
        items.extend([
            MenuItem::Separator,
            MenuAction::new("window", "Open in New Window")
                .submenu(
                    self.workspace
                        .vorkspaces
                        .iter()
                        .map(|def| {
                            MenuAction::new(format!("new-window:{}", def.id), def.name.clone())
                        })
                        .collect(),
                )
                .into(),
            // No icon, so it lines up with the vorkspace names above.
            MenuAction::new("manage", "Manage vorkspaces").into(),
        ]);
        ("Vorkspaces".into(), items, 240., MenuStyle::Group)
    }

    pub(super) fn run_vorkspace_menu(
        &mut self,
        action: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = action.strip_prefix("switch:") {
            let id = id.to_owned();
            self.switch_vorkspace(&id, window, cx);
        } else if let Some(id) = action.strip_prefix("new-window:") {
            self.new_window(Some(id), window, cx);
        } else if action == "manage" {
            self.open_vorkspace_dialog(None, window, cx);
        }
    }

    pub(super) fn configure_vorkspace_bots(&self, cx: &mut Context<Self>) {
        let Some(manager) = self.bots.clone() else {
            return;
        };
        let hidden = self.workspace.hidden_bots();
        let active = self.workspace.active_vorkspace.clone();
        manager.update(cx, |m, _| m.set_vorkspace(active, hidden));
    }

    /// The vorkspace switcher at the top of the rail.
    pub(super) fn vorkspace_switcher(&self, cx: &mut Context<Self>) -> AnyElement {
        let (_, name) = self
            .workspace
            .active_vorkspace()
            .map(|v| (v.id.clone(), v.name.clone()))
            .unwrap_or_default();
        div()
            .px_2()
            .pb_1()
            .child(
                div()
                    .id("vorkspace-switcher")
                    .debug_selector(|| "vorkspace-switcher".into())
                    .role(Role::Button)
                    .aria_label(format!("Vorkspace {name}. Switch vorkspace"))
                    .map(vorkspace_ui::theme::tab_reachable)
                    .relative()
                    .h(px(36.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(radius::CONTROL))
                    .cursor_pointer()
                    .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.08)))
                    .child(vorkspace_mark(
                        &self
                            .workspace
                            .active_vorkspace()
                            .cloned()
                            .unwrap_or_default(),
                        22.,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(name),
                    )
                    .child(icon("chevron-down", 12.).text_color(content(0.45)))
                    .child(self.vorkspace_anchor.probe())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_menu(
                            MenuTarget::Vorkspace,
                            Anchor::Slot(this.vorkspace_anchor.clone()),
                            window,
                            cx,
                        );
                    })),
            )
            .into_any_element()
    }

    pub(super) fn sync_vorkspace_bot_visibility(&mut self, cx: &mut Context<Self>) -> bool {
        let hidden = self.workspace.hidden_bots();
        let active = self.workspace.active_vorkspace.clone();
        if let Some(manager) = self.bots.clone() {
            manager.update(cx, |m, cx| {
                if m.active.as_ref().is_some_and(|id| hidden.contains(id)) {
                    m.close(cx);
                }
                m.set_vorkspace(active, hidden);
                cx.notify();
            });
        }
        let Some(active) = self.workspace.active else {
            return false;
        };
        if !self
            .workspace
            .app_panes
            .get(&active)
            .is_some_and(|pane| match &pane.content {
                crate::workspace::AppPane::Bot(id)
                | crate::workspace::AppPane::BotDocument { bot: id, .. } => {
                    self.workspace.bot_hidden_in_vorkspace(id)
                }
                _ => false,
            })
        {
            return false;
        }
        let next = self
            .workspace
            .active_tab()
            .and_then(|tab| self.available_tab_focus(tab, cx))
            .or_else(|| {
                self.visible_tabs(cx)
                    .iter()
                    .rev()
                    .find_map(|tab| self.available_tab_focus(tab.session()?, cx))
            });
        self.workspace.active = None;
        if let Some(next) = next {
            self.workspace.open(next);
        }
        self.review_focused = false;
        self.update_chat_visibility(cx);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use vorkspace_core::files::path_key;

    #[gpui::test]
    fn history_switches_vorkspace_with_the_conversation(cx: &mut TestAppContext) {
        let (window, a, b) = super::super::tests::window(cx);
        window
            .update(cx, |shell, window, cx| {
                let main = shell.workspace.active_vorkspace.clone();
                let side = shell.workspace.create_vorkspace("Side");
                let project = "/work/another-project";
                shell
                    .workspace
                    .remove_project_from_vorkspace(&main, project);
                shell.workspace.add_project_to_vorkspace(&side, project);

                shell.open(a, window, cx);
                assert_eq!(shell.workspace.active_vorkspace, side);
                assert_eq!(shell.workspace.project, project);
                shell.open(b, window, cx);
                assert_eq!(shell.workspace.active_vorkspace, main);
                shell.visit(-1, window, cx);
                assert_eq!(shell.workspace.active_vorkspace, side);
                assert_eq!(shell.workspace.active, Some(a));
                shell.visit(1, window, cx);
                assert_eq!(shell.workspace.active_vorkspace, main);
                assert_eq!(shell.workspace.active, Some(b));
            })
            .unwrap();
    }

    #[gpui::test]
    fn managing_vorkspaces_keeps_new_session_shortcut_out_of_the_chat(cx: &mut TestAppContext) {
        cx.update(super::super::bind_keys);
        let (window, _, b) = super::super::tests::window(cx);
        let before = window
            .update(cx, |shell, w, cx| {
                shell.open(b, w, cx);
                shell.open_vorkspace_dialog(None, w, cx);
                shell.order.len()
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.simulate_keystrokes("cmd-n");
        visual.run_until_parked();
        window
            .update(visual, |shell, w, cx| {
                assert_eq!(shell.order.len(), before);
                assert_eq!(shell.workspace.active, Some(b));
                assert!(
                    shell
                        .management_modal_focus(cx)
                        .unwrap()
                        .contains_focused(w, cx)
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn managing_many_folders_scrolls_the_focused_project_into_view(cx: &mut TestAppContext) {
        cx.update(super::super::bind_keys);
        let (window, _, _) = super::super::tests::window(cx);
        window
            .update(cx, |shell, w, cx| {
                let editing = shell.workspace.active_vorkspace.clone();
                for n in 0..40 {
                    shell
                        .workspace
                        .add_project_to_vorkspace(&editing, &format!("/work/folder-{n}"));
                }
                shell.open_vorkspace_dialog(None, w, cx);
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.simulate_resize(gpui::size(px(700.), px(480.)));
        visual.run_until_parked();
        // Move from the name field backwards past the fixed New button.
        visual.simulate_keystrokes("shift-tab shift-tab");
        visual.run_until_parked();
        let nav = visual.debug_bounds("vorkspace-dialog-nav").unwrap();
        let last = visual
            .debug_bounds("vorkspace-project-tab-/work/folder-39")
            .unwrap();
        let add = visual.debug_bounds("vorkspace-new").unwrap();
        assert!(
            last.top() >= nav.top() && last.bottom() <= nav.bottom(),
            "{last:?} outside {nav:?}"
        );
        assert!(add.top() >= nav.bottom());
        visual.simulate_event(gpui::KeyDownEvent {
            keystroke: gpui::Keystroke::parse("enter").unwrap(),
            is_held: false,
            prefer_character_input: false,
        });
        visual.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("enter").unwrap(),
        });
        window
            .update(visual, |shell, _, _| {
                assert_eq!(
                    shell.vorkspace_dialog.as_ref().unwrap().project.as_deref(),
                    Some("/work/folder-39")
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn adding_first_folder_chip_to_active_empty_vorkspace_enters_that_project(
        cx: &mut TestAppContext,
    ) {
        let (window, _, _) = super::super::tests::window(cx);
        let second_project = "/work/second".to_string();
        window
            .update(cx, |shell, window, cx| {
                shell.workspace.add_project(&second_project);
                let empty_id = shell.workspace.create_vorkspace("Empty");
                shell.switch_vorkspace(&empty_id, window, cx);
                assert_eq!(shell.workspace.active_vorkspace, empty_id);
                assert!(shell.workspace.project.is_empty());
                assert!(shell.workspace.vorkspace_projects().is_empty());

                shell.open_vorkspace_dialog(Some(empty_id.clone()), window, cx);
            })
            .unwrap();

        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let chip_id: &'static str =
            format!("vorkspace-add-project-{}", path_key(&second_project)).leak();
        let chip_bounds = visual
            .debug_bounds(chip_id)
            .expect("add project chip rendered");
        visual.simulate_click(chip_bounds.center(), Default::default());
        visual.run_until_parked();

        window
            .update(visual, |shell, _, _| {
                assert_eq!(shell.workspace.project, second_project);
                assert_eq!(shell.workspace.vorkspace_projects(), [second_project]);
            })
            .unwrap();
    }
}
