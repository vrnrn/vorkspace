//! Management dialog for Vorkspace appearance, folders and Bot membership.
use super::vorkspaces::vorkspace_mark;
use super::*;
use vorkspace_core::files::path_key;
use vorkspace_ui::radius;

pub(crate) struct VorkspaceDialog {
    pub(crate) editing: String,
    /// A project's own tab (Codebase, Notifications); none shows the
    /// vorkspace itself.
    pub(crate) project: Option<String>,
    pub(super) name: Entity<input::Input>,
    pub(super) icon_letters: Entity<input::Input>,
    pub(super) focus: FocusHandle,
    pub(super) confirm_delete: bool,
    _fields: [Subscription; 2],
}

impl Shell {
    pub(super) fn open_vorkspace_dialog(
        &mut self,
        id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = id
            .filter(|id| self.workspace.vorkspaces.iter().any(|v| &v.id == id))
            .unwrap_or_else(|| self.workspace.active_vorkspace.clone());
        let (name, letters) = self
            .workspace
            .vorkspaces
            .iter()
            .find(|v| v.id == id)
            .map(|v| (v.name.clone(), v.icon_letters().to_owned()))
            .unwrap_or_default();
        let name = cx.new(|cx| {
            let mut field =
                input::Input::field("Vorkspace name", "Vorkspace name", content(0.35), cx);
            field.set_text(name, cx);
            field
        });
        let name_sub = cx.subscribe(&name, |this, field, _: &input::Changed, cx| {
            let Some(editing) = this
                .project_settings
                .vorkspace_dialog
                .as_ref()
                .filter(|d| d.name == field)
                .map(|d| &d.editing)
            else {
                return;
            };
            let name = field.read(cx).content.trim().to_owned();
            if name.is_empty() {
                return;
            }
            if let Some(v) = this.workspace.vorkspace_mut(editing)
                && v.name != name
            {
                v.name = name;
                this.persist(cx);
                cx.notify();
            }
        });
        let icon_letters = cx.new(|cx| {
            let mut field = input::Input::field("Icon letters", "Auto", content(0.45), cx);
            field.set_centered();
            field.set_text(letters, cx);
            field
        });
        let icon_sub = cx.subscribe(&icon_letters, |this, field, _: &input::Changed, cx| {
            let input = field.read(cx);
            if input.is_composing() {
                return;
            }
            let Some(editing) = this
                .project_settings
                .vorkspace_dialog
                .as_ref()
                .filter(|d| d.icon_letters == field)
                .map(|d| &d.editing)
            else {
                return;
            };
            let text = input.content.clone();
            let Some(v) = this.workspace.vorkspace_mut(editing) else {
                return;
            };
            let changed = v.set_icon_letters(&text);
            if text.as_ref() != v.icon_letters() {
                let canonical = v.icon_letters().to_owned();
                field.update(cx, |field, cx| field.set_text(canonical, cx));
            }
            if changed {
                this.persist(cx);
                cx.notify();
            }
        });
        let focus = cx.focus_handle();
        self.navigation_revision += 1;
        window.focus(&name.focus_handle(cx), cx);
        self.project_settings.vorkspace_dialog = Some(VorkspaceDialog {
            editing: id,
            project: None,
            name,
            icon_letters,
            focus,
            confirm_delete: false,
            _fields: [name_sub, icon_sub],
        });
        cx.notify();
    }

    pub(super) fn close_vorkspace_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project_settings.vorkspace_dialog.take().is_some() {
            self.focus_workspace(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn management_modal_focus(&self, cx: &App) -> Option<FocusHandle> {
        self.project_settings
            .background_dialog
            .as_ref()
            .map(|background| background.view.focus_handle(cx))
            .or_else(|| {
                self.project_settings
                    .vorkspace_dialog
                    .as_ref()
                    .map(|dialog| dialog.focus.clone())
            })
    }

    fn vorkspace_bots(&self, cx: &App) -> Vec<vorkspace_core::bot::Bot> {
        self.app_core
            .as_ref()
            .and_then(|core| core.read(cx).bot_roster.read(cx).committed())
            .map(|roster| {
                roster
                    .bots
                    .iter()
                    .filter(|b| !b.is_minion())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn vorkspace_modal(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.project_settings.vorkspace_dialog.as_ref()?;
        let editing = dialog.editing.clone();
        let def = self
            .workspace
            .vorkspaces
            .iter()
            .find(|v| v.id == editing)?
            .clone();
        let size = window.viewport_size();
        let width = 720f32.min(size.width.as_f32() - 24.);
        let height = 560f32.min(size.height.as_f32() - 32.);
        let weak = cx.weak_entity();
        let look = self.vorkspace_look(&def, &dialog.icon_letters, cx);
        let open_project = dialog.project.clone();

        let project_page = open_project
            .as_ref()
            .map(|path| self.vorkspace_project_page(path, cx));
        let (nav, scroll) =
            self.vorkspace_dialog_navigation(&editing, open_project.as_deref(), window, cx);
        let body = self.vorkspace_dialog_body(dialog, &def, look, project_page, cx);
        Some(
            div()
                .absolute()
                .size_full()
                .child(
                    gpui::deferred(
                        gpui::anchored()
                            .position(gpui::point(px(0.), px(0.)))
                            .child(
                                div()
                                    .w(size.width)
                                    .h(size.height)
                                    .bg(vorkspace_ui::fluid::scrim(0.4))
                                    .occlude(),
                            ),
                    )
                    .with_priority(59),
                )
                .child(
                    popover(
                        "vorkspace-modal",
                        Anchor::Point(gpui::point(
                            px((size.width.as_f32() - width) / 2.),
                            px((size.height.as_f32() - height) / 2.),
                        )),
                    )
                    .modal_surface()
                    .without_animation()
                    .priority(60)
                    .width(width)
                    .height(height)
                    .gap(0.)
                    .on_dismiss(move |window, cx| {
                        let _ = weak.update(cx, |this, cx| this.close_vorkspace_dialog(window, cx));
                    })
                    .child(
                        div()
                            .id("vorkspace-dialog")
                            .debug_selector(|| "vorkspace-dialog".into())
                            .role(Role::Dialog)
                            .aria_label("Vorkspaces")
                            .text_color(content(1.))
                            .track_focus(&dialog.focus)
                            .on_key_down(cx.listener(
                                |this, event: &gpui::KeyDownEvent, window, cx| {
                                    if event.keystroke.key == "escape" {
                                        this.close_vorkspace_dialog(window, cx);
                                        cx.stop_propagation();
                                    }
                                },
                            ))
                            .size_full()
                            .flex()
                            .child(self.vorkspace_navigation_frame(nav, scroll, cx))
                            .child(body),
                    ),
                )
                .into_any_element(),
        )
    }

    fn vorkspace_dialog_section(label: &'static str) -> gpui::Div {
        div()
            .mt_5()
            .mb_2()
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(content(0.55))
            .child(label)
    }

    fn vorkspace_dialog_chip(
        id: SharedString,
        label: String,
        lead: AnyElement,
    ) -> gpui::Stateful<gpui::Div> {
        let debug_id = id.clone();
        div()
            .id(id)
            .debug_selector(move || debug_id.to_string())
            .role(Role::Button)
            .aria_label(format!("Add {label}"))
            .map(vorkspace_ui::theme::tab_reachable)
            .h(px(28.))
            .px_2()
            .flex()
            .items_center()
            .gap_1p5()
            .rounded_full()
            .border_1()
            .border_color(content(0.12))
            .text_size(px(12.))
            .text_color(content(0.75))
            .cursor_pointer()
            .hover(|s| {
                s.bg(vorkspace_ui::fluid::hover(0.06))
                    .text_color(content(1.))
            })
            .child(icon("plus", 12.))
            .child(lead)
            .child(label)
    }

    fn vorkspace_project_page(&self, path: &str, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .mb_1()
                    .child(self.project_mark(path, 18.))
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.project_label(path)),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(content(0.45))
                    .child(crate::panel_kit::home_relative(std::path::Path::new(path))),
            )
            .child(
                div()
                    .mt_5()
                    .mb_2()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(content(0.55))
                    .child("Codebase"),
            )
            .child(self.project_codebase_section(path, cx))
            .child(
                div()
                    .mt_5()
                    .mb_2()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(content(0.55))
                    .child("Notifications"),
            )
            .child(self.project_notifications_section(path, cx))
            .into_any_element()
    }

    fn vorkspace_navigation_row(
        &self,
        v: &crate::workspace::VorkspaceDef,
        selected: bool,
        focus: FocusHandle,
        id: String,
        cx: &Context<Self>,
    ) -> AnyElement {
        div()
            .id(SharedString::from(format!("vorkspace-row-{}", v.id)))
            .debug_selector({
                let id = v.id.clone();
                move || format!("vorkspace-row-{id}")
            })
            .role(Role::Tab)
            .aria_selected(selected)
            .aria_label(v.name.clone())
            .map(vorkspace_ui::theme::tab_reachable)
            .track_focus(&focus)
            .h(px(34.))
            .flex_shrink_0()
            .px(px(8.))
            .flex()
            .items_center()
            .gap_2()
            .rounded(px(radius::CONTROL))
            .text_size(px(13.))
            .text_color(content(if selected { 1. } else { 0.7 }))
            .cursor_pointer()
            .when(selected, |d| d.bg(content(0.08)))
            .when(!selected, |d| {
                d.hover(|s| s.bg(vorkspace_ui::fluid::hover(0.05)))
            })
            .child(vorkspace_mark(v, 18.))
            .child(div().flex_1().min_w_0().truncate().child(v.name.clone()))
            .when(v.id == self.workspace.active_vorkspace, |d| {
                d.child(icon("check-2", 12.).text_color(content(0.45)))
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                if this
                    .project_settings
                    .vorkspace_dialog
                    .as_ref()
                    .is_some_and(|d| d.editing == id)
                {
                    if let Some(d) = this.project_settings.vorkspace_dialog.as_mut() {
                        d.project = None;
                        d.confirm_delete = false;
                    }
                    cx.notify();
                } else {
                    this.open_vorkspace_dialog(Some(id.clone()), window, cx);
                }
            }))
            .into_any_element()
    }

    fn vorkspace_project_tab(
        &self,
        path: &str,
        key: String,
        selected: bool,
        focus: FocusHandle,
        cx: &Context<Self>,
    ) -> AnyElement {
        let target = path.to_owned();
        div()
            .id(SharedString::from(format!("vorkspace-project-tab-{key}")))
            .debug_selector(move || format!("vorkspace-project-tab-{key}"))
            .role(Role::Tab)
            .aria_selected(selected)
            .aria_label(format!("{} settings", self.project_label(path)))
            .map(vorkspace_ui::theme::tab_reachable)
            .track_focus(&focus)
            .h(px(30.))
            .flex_shrink_0()
            .pl(px(20.))
            .pr(px(8.))
            .flex()
            .items_center()
            .gap_2()
            .rounded(px(radius::CONTROL))
            .text_size(px(12.5))
            .text_color(content(if selected { 1. } else { 0.65 }))
            .cursor_pointer()
            .when(selected, |d| d.bg(content(0.08)))
            .when(!selected, |d| {
                d.hover(|s| s.bg(vorkspace_ui::fluid::hover(0.05)))
            })
            .child(self.project_mark(path, 14.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(self.project_label(path)),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(d) = this.project_settings.vorkspace_dialog.as_mut() {
                    d.project = Some(target.clone());
                    d.confirm_delete = false;
                }
                cx.notify();
            }))
            .into_any_element()
    }

    fn vorkspace_dialog_navigation(
        &self,
        editing: &str,
        open_project: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Vec<AnyElement>, gpui::ScrollHandle) {
        let nav_state = window.use_keyed_state("vorkspace-dialog-navigation", cx, |_, _| {
            (
                gpui::ScrollHandle::new(),
                std::cell::RefCell::<Option<FocusHandle>>::default(),
            )
        });
        let focused = window.focused(cx);
        let (scroll, previous_focus) = nav_state.read(cx);
        let focus_changed = previous_focus.replace(focused.clone()) != focused;
        let scroll = scroll.clone();
        let nav_focus = |key: SharedString, ix, window: &mut Window, cx: &mut App| {
            let handle = window
                .use_keyed_state(key, cx, |_, cx| cx.focus_handle().tab_stop(true))
                .read(cx)
                .clone();
            if focus_changed && focused.as_ref() == Some(&handle) {
                scroll.scroll_to_item(ix);
            }
            handle
        };
        let mut nav: Vec<AnyElement> = Vec::new();
        for v in self.workspace.vorkspaces.iter() {
            let id = v.id.clone();
            let current = v.id == editing;
            let selected = current && open_project.is_none();
            let focus = nav_focus(format!("vorkspace-nav-{id}").into(), nav.len(), window, cx);
            nav.push(self.vorkspace_navigation_row(v, selected, focus, id, cx));
            if !current {
                continue;
            }
            // Each of the vorkspace's projects has its own settings tab.
            for path in v
                .projects
                .iter()
                .filter(|p| self.workspace.is_project_available(p))
            {
                let selected = open_project == Some(path.as_str());
                let key = path_key(path);
                let focus = nav_focus(
                    format!("vorkspace-nav-project-{}-{key}", v.id).into(),
                    nav.len(),
                    window,
                    cx,
                );
                nav.push(self.vorkspace_project_tab(path, key, selected, focus, cx));
            }
        }

        (nav, scroll)
    }

    fn vorkspace_project_row(
        &self,
        path: String,
        editing: String,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let remove = path.clone();

        let key = path_key(&path);
        div()
            .id(SharedString::from(format!("vorkspace-project-{key}")))
            .h(px(40.))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .rounded(px(radius::CONTROL))
            .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.04)))
            .child(self.project_mark(&path, 16.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(13.))
                            .child(self.project_label(&path)),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(content(0.45))
                            .child(crate::panel_kit::home_relative(std::path::Path::new(&path))),
                    ),
            )
            .child(
                icon_button(
                    "vorkspace-remove-project",
                    "Remove from this vorkspace",
                    "close",
                )
                .aria_label(format!(
                    "Remove {} from this vorkspace",
                    self.project_label(&path)
                ))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.workspace
                        .remove_project_from_vorkspace(&editing, &remove);
                    if editing == this.workspace.active_vorkspace
                        && path_key(&remove) == path_key(&this.workspace.project)
                    {
                        let landing = this.workspace.vorkspace_landing(&editing);
                        this.open_project_from_user(landing, window, cx);
                    } else {
                        this.persist(cx);
                        cx.notify();
                    }
                })),
            )
    }

    fn vorkspace_add_project(
        &self,
        path: String,
        editing: String,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let add = path.clone();
        Self::vorkspace_dialog_chip(
            SharedString::from(format!("vorkspace-add-project-{}", path_key(&path))),
            self.project_label(&path),
            self.project_mark(&path, 14.),
        )
        .on_click(cx.listener(move |this, _, window, cx| {
            let first = editing == this.workspace.active_vorkspace
                && (this.workspace.project.is_empty()
                    || !this
                        .workspace
                        .vorkspace_projects()
                        .iter()
                        .any(|p| path_key(p) == path_key(&this.workspace.project)));
            this.workspace.add_project_to_vorkspace(&editing, &add);
            if first {
                this.open_project_from_user(add.clone(), window, cx);
            } else {
                this.persist(cx);
                cx.notify();
            }
        }))
    }

    fn vorkspace_folder_fields(
        &self,
        d: gpui::Stateful<gpui::Div>,
        def: &crate::workspace::VorkspaceDef,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let editing = def.id.clone();
        let projects = def
            .projects
            .clone()
            .into_iter()
            .filter(|p| self.workspace.is_project_available(p))
            .map(|path| self.vorkspace_project_row(path, editing.clone(), cx));
        // Folders already known elsewhere can join with one click.
        let addable: Vec<String> = self
            .workspace
            .projects
            .iter()
            .filter(|p| {
                self.workspace.is_project_available(p)
                    && !def.projects.iter().any(|q| path_key(q) == path_key(p))
            })
            .cloned()
            .collect();
        let add_projects = addable
            .into_iter()
            .map(|path| self.vorkspace_add_project(path, editing.clone(), cx));

        d.child(Self::vorkspace_dialog_section("Folders"))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(content(0.5))
                    .mb_2()
                    .child("A folder can be in several vorkspaces; its sessions show in each."),
            )
            .children(projects)
            .child(
                div()
                    .mt_2()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Self::vorkspace_dialog_chip(
                            "vorkspace-add-folder".into(),
                            "Add folder…".into(),
                            div().into_any_element(),
                        )
                        .debug_selector(|| "vorkspace-add-folder".into())
                        .on_click(cx.listener(|this, _, _, cx| this.open_project(cx))),
                    )
                    .children(add_projects),
            )
    }

    fn vorkspace_member_row(
        &self,
        bot: vorkspace_core::bot::Bot,
        editing: String,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let id = bot.id.clone();
        div()
            .id(SharedString::from(format!("vorkspace-bot-{}", bot.id)))
            .h(px(36.))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .rounded(px(radius::CONTROL))
            .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.04)))
            .child(crate::avatar::bot_avatar(&bot, "idle", 18., true))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .child(bot.label()),
            )
            .child(
                icon_button(
                    "vorkspace-remove-bot",
                    "Remove from this vorkspace",
                    "close",
                )
                .aria_label(format!("Remove {} from this vorkspace", bot.label()))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_bot_membership(&id, &editing, false, cx);
                    this.reconcile_after_bot_move(window, cx);
                })),
            )
    }

    fn vorkspace_add_bot(
        &self,
        bot: vorkspace_core::bot::Bot,
        editing: String,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let id = bot.id.clone();
        Self::vorkspace_dialog_chip(
            SharedString::from(format!("vorkspace-add-bot-{}", bot.id)),
            bot.label(),
            crate::avatar::bot_avatar(&bot, "idle", 14., true),
        )
        .on_click(cx.listener(move |this, _, window, cx| {
            this.set_bot_membership(&id, &editing, true, cx);
            this.reconcile_after_bot_move(window, cx);
        }))
    }

    fn vorkspace_bot_fields(
        &self,
        d: gpui::Stateful<gpui::Div>,
        editing: &str,
        members: Vec<vorkspace_core::bot::Bot>,
        others: Vec<vorkspace_core::bot::Bot>,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let editing = editing.to_owned();
        let no_bots = members.is_empty() && others.is_empty();
        let member_rows = members
            .into_iter()
            .map(|bot| self.vorkspace_member_row(bot, editing.clone(), cx));
        let add_bots = others
            .into_iter()
            .map(|bot| self.vorkspace_add_bot(bot, editing.clone(), cx));

        d.child(Self::vorkspace_dialog_section("Bots"))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(content(0.5))
                    .mb_2()
                    .child(if no_bots {
                        "A Bot created while this vorkspace is open belongs to it."
                    } else {
                        "A Bot belongs to one vorkspace; adding one here moves it."
                    }),
            )
            .children(member_rows)
            .child(div().mt_2().flex().flex_wrap().gap_2().children(add_bots))
    }

    fn vorkspace_delete_button(
        &self,
        dialog: &VorkspaceDialog,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let confirm = dialog.confirm_delete;
        let only = self.workspace.vorkspaces.len() <= 1;
        let delete_id = dialog.editing.clone();
        crate::controls::page::secondary(
            "vorkspace-delete",
            if confirm {
                "Confirm delete"
            } else {
                "Delete vorkspace"
            },
            true,
            only,
        )
        .debug_selector(|| "vorkspace-delete".into())
        .child(if confirm {
            "Confirm delete"
        } else {
            "Delete vorkspace"
        })
        .on_click(cx.listener(move |this, _, window, cx| {
            if only {
                return;
            }
            let Some(d) = this.project_settings.vorkspace_dialog.as_mut() else {
                return;
            };
            if d.project.is_some() || d.editing != delete_id {
                return;
            }
            if !d.confirm_delete {
                d.confirm_delete = true;
                cx.notify();
                return;
            }
            let was_active = delete_id == this.workspace.active_vorkspace;
            if this.workspace.delete_vorkspace(&delete_id) {
                if was_active {
                    let landing = this
                        .workspace
                        .vorkspace_landing(&this.workspace.active_vorkspace.clone());
                    this.open_project_from_user(landing, window, cx);
                }
                let next = this.workspace.active_vorkspace.clone();
                this.configure_vorkspace_bots(cx);
                this.persist(cx);
                this.open_vorkspace_dialog(Some(next), window, cx);
            }
        }))
    }

    fn vorkspace_switch_button(
        &self,
        dialog: &VorkspaceDialog,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let switch_id = dialog.editing.clone();
        crate::controls::page::secondary(
            "vorkspace-switch-here",
            "Switch to this vorkspace",
            false,
            false,
        )
        .child("Switch to it")
        .on_click(cx.listener(move |this, _, window, cx| {
            this.switch_vorkspace(&switch_id, window, cx);
            this.open_vorkspace_dialog(Some(switch_id.clone()), window, cx);
        }))
    }

    fn vorkspace_dialog_footer(&self, dialog: &VorkspaceDialog, cx: &Context<Self>) -> gpui::Div {
        let open_project = &dialog.project;
        let is_active = dialog.editing == self.workspace.active_vorkspace;
        div()
            .flex_shrink_0()
            .px_5()
            .py_3()
            .border_t_1()
            .border_color(content(0.06))
            .flex()
            .items_center()
            .gap_2()
            .when(open_project.is_none(), |d| {
                d.child(self.vorkspace_delete_button(dialog, cx))
            })
            .child(div().flex_1())
            .when(open_project.is_none() && !is_active, |d| {
                d.child(self.vorkspace_switch_button(dialog, cx))
            })
            .child(
                crate::controls::page::primary("vorkspace-done", "Done", false)
                    .debug_selector(|| "vorkspace-done".into())
                    .child("Done")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.close_vorkspace_dialog(window, cx)),
                    ),
            )
    }

    fn vorkspace_dialog_body(
        &self,
        dialog: &VorkspaceDialog,
        def: &crate::workspace::VorkspaceDef,
        look: AnyElement,
        project_page: Option<AnyElement>,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let open_project = &dialog.project;
        let bots = self.vorkspace_bots(cx);
        let (members, others): (Vec<_>, Vec<_>) =
            bots.into_iter().partition(|b| def.bots.contains(&b.id));
        div()
            .id("vorkspace-dialog-body")
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("vorkspace-dialog-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_5()
                    .pt_4()
                    .pb_4()
                    .children(project_page)
                    .when(open_project.is_none(), |d| {
                        d.child(Self::vorkspace_dialog_section("Name").mt_0())
                            .child(crate::controls::page::field_frame().child(dialog.name.clone()))
                            .child(Self::vorkspace_dialog_section("Look"))
                            .child(look)
                            .map(|d| self.vorkspace_folder_fields(d, def, cx))
                            .map(|d| {
                                self.vorkspace_bot_fields(d, &dialog.editing, members, others, cx)
                            })
                    }),
            )
            .child(self.vorkspace_dialog_footer(dialog, cx))
    }

    fn vorkspace_navigation_frame(
        &self,
        nav: Vec<AnyElement>,
        scroll: gpui::ScrollHandle,
        cx: &Context<Self>,
    ) -> gpui::Div {
        div()
            .w(px(200.))
            .flex_shrink_0()
            .h_full()
            .border_r_1()
            .border_color(content(0.04))
            .p_2()
            .flex()
            .flex_col()
            .gap_px()
            .child(
                div()
                    .px_2()
                    .pt_1()
                    .pb_2()
                    .text_size(px(12.))
                    .text_color(content(0.5))
                    .child("Vorkspaces"),
            )
            .child(
                div()
                    .id("vorkspace-dialog-nav")
                    .debug_selector(|| "vorkspace-dialog-nav".into())
                    .role(Role::TabList)
                    .aria_label("Vorkspaces")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .flex()
                    .flex_col()
                    .gap_px()
                    .children(nav),
            )
            .child(
                crate::controls::page::action("vorkspace-new", "New vorkspace", true)
                    .debug_selector(|| "vorkspace-new".into())
                    .child(icon("plus", 12.))
                    .child("New vorkspace")
                    .on_click(cx.listener(|this, _, window, cx| {
                        let n = this.workspace.vorkspaces.len() + 1;
                        let id = this.workspace.create_vorkspace(&format!("Vorkspace {n}"));
                        this.persist(cx);
                        this.open_vorkspace_dialog(Some(id), window, cx);
                    })),
            )
    }
    /// The vorkspace's mark and the chat background its sessions show.
    fn vorkspace_look(
        &self,
        def: &crate::workspace::VorkspaceDef,
        letters: &Entity<input::Input>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let swatches = vorkspace_core::project_appearance::COLORS
            .iter()
            .skip(1)
            .copied()
            .map(|color| {
                let selected = def.color == Some(color);
                let editing = def.id.clone();
                div()
                    .id(SharedString::from(format!("vorkspace-color-{color:06x}")))
                    .role(Role::Button)
                    .aria_label(format!("Color #{color:06x}"))
                    .aria_selected(selected)
                    .map(vorkspace_ui::theme::tab_reachable)
                    .size(px(26.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .when(selected, |d| d.border_2().border_color(content(0.7)))
                    .child(crate::avatar::color_swatch_circle(color, None, 18.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(v) = this.workspace.vorkspace_mut(&editing) {
                            v.color = (v.color != Some(color)).then_some(color);
                        }
                        this.invalidate_chrome(cx);
                        this.persist(cx);
                        cx.notify();
                    }))
            });
        let editing = def.id.clone();
        let has_background = def.background.is_some();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(content(0.75))
                            .child("Icon letters"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(vorkspace_mark(def, 26.))
                            .child(
                                crate::controls::page::field_frame()
                                    .w(px(64.))
                                    .flex_shrink_0()
                                    .child(letters.clone()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(12.))
                                    .text_color(content(0.55))
                                    .child("Up to two letters. Clear to use the name’s initial."),
                            ),
                    ),
            )
            .child(div().flex().flex_wrap().gap_1().children(swatches))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        crate::controls::page::secondary(
                            "vorkspace-background",
                            "Chat background",
                            false,
                            false,
                        )
                        .debug_selector(|| "vorkspace-background".into())
                        .child(if has_background {
                            "Change chat background…"
                        } else {
                            "Chat background…"
                        })
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.open_vorkspace_background(editing.clone(), window, cx)
                            },
                        )),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(content(0.5))
                            .child("Shown behind every session in this vorkspace."),
                    ),
            )
            .into_any_element()
    }

    fn reconcile_after_bot_move(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sync_vorkspace_bot_visibility(cx) {
            if let Some(active) = self.workspace.active {
                self.open(active, window, cx);
            } else {
                self.ensure_project_session(window, cx);
            }
        }
        self.persist(cx);
        cx.notify();
    }
}
