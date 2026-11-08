use super::*;
use crate::settings::ProjectNotifications;
use crate::{
    project_background::ProjectBackground,
    project_settings::{AppearanceControls, BackgroundDialog, Event, project_icon},
};
use vorkspace_core::project_appearance::ProjectAppearance;

/// A section of Project settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProjectSection {
    Appearance,
    Background,
    Codebase,
    Notifications,
}

impl ProjectSection {
    const ALL: [Self; 4] = [
        Self::Appearance,
        Self::Background,
        Self::Codebase,
        Self::Notifications,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Background => "Background",
            Self::Codebase => "Codebase",
            Self::Notifications => "Notifications",
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Self::Appearance => "palette",
            Self::Background => "image-add",
            Self::Codebase => "network",
            Self::Notifications => "bell",
        }
    }
}

/// The open Project settings dialog. Only the shown section's editor
/// exists: each one holds its own copy of the project's appearance, so
/// building it fresh from the saved appearance when it is shown keeps one
/// editor's save from overwriting another's.
pub(crate) struct ProjectDialog {
    path: String,
    section: ProjectSection,
    body: Option<gpui::AnyView>,
    focus: FocusHandle,
    _events: Option<Subscription>,
}

pub(super) struct OpenBackground {
    view: Entity<BackgroundDialog>,
    _events: Subscription,
}
impl Shell {
    pub(super) fn appearance(&self, path: &str) -> ProjectAppearance {
        self.workspace
            .project_appearance
            .get(path)
            .cloned()
            .unwrap_or_default()
    }
    pub(super) fn project_label(&self, path: &str) -> String {
        self.appearance(path).label(path)
    }
    pub(super) fn project_mark(&self, path: &str, side: f32) -> AnyElement {
        project_icon(&self.appearance(path), path, side)
    }
    fn save_project_appearance(
        &mut self,
        path: String,
        mut appearance: ProjectAppearance,
        cx: &mut Context<Self>,
    ) {
        appearance.normalize();
        for session in self.sessions.values() {
            if session.read(cx).summary.cwd == path {
                session.update(cx, |session, cx| {
                    session.set_project_appearance(appearance.clone(), cx)
                });
            }
        }
        self.workspace.project_appearance.insert(path, appearance);
        self.invalidate_chrome(cx);
        self.persist(cx);
        cx.notify();
    }
    pub(super) fn open_project_settings(
        &mut self,
        path: String,
        section: ProjectSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_menu(cx);
        self.close_preview(cx);
        self.project_dialog = Some(ProjectDialog {
            path,
            section,
            body: None,
            focus: cx.focus_handle(),
            _events: None,
        });
        self.show_project_section(section, window, cx);
    }

    fn show_project_section(
        &mut self,
        section: ProjectSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.project_dialog.as_ref().map(|d| d.path.clone()) else {
            return;
        };
        let appearance = self.appearance(&path);
        let on_event = {
            let path = path.clone();
            move |this: &mut Shell, event: &Event, window: &mut Window, cx: &mut Context<Shell>| {
                match event {
                    Event::Changed(appearance) => {
                        this.save_project_appearance(path.clone(), appearance.clone(), cx)
                    }
                    Event::Close => this.close_project_settings(window, cx),
                }
            }
        };
        let (body, focus, events): (Option<gpui::AnyView>, Option<FocusHandle>, _) = match section {
            ProjectSection::Appearance => {
                let view = cx.new(|cx| AppearanceControls::new(path.clone(), appearance, cx));
                let focus = view.read(cx).focus(cx);
                let events = cx.subscribe_in(&view, window, move |this, _, event, window, cx| {
                    on_event(this, event, window, cx)
                });
                (Some(view.into()), Some(focus), Some(events))
            }
            ProjectSection::Background => {
                let view =
                    cx.new(|cx| BackgroundDialog::new(path.clone(), appearance, cx).embedded());
                let focus = view.focus_handle(cx);
                let events = cx.subscribe_in(&view, window, move |this, _, event, window, cx| {
                    on_event(this, event, window, cx)
                });
                (Some(view.into()), Some(focus), Some(events))
            }
            ProjectSection::Codebase | ProjectSection::Notifications => (None, None, None),
        };
        if let Some(dialog) = self.project_dialog.as_mut() {
            dialog.section = section;
            dialog.body = body;
            dialog._events = events;
            window.focus(focus.as_ref().unwrap_or(&dialog.focus), cx);
        }
        cx.notify();
    }

    pub(super) fn close_project_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project_dialog.take().is_some() {
            self.focus_workspace(window, cx);
            cx.notify();
        }
    }

    pub(super) fn project_settings_modal(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.project_dialog.as_ref()?;
        let size = window.viewport_size();
        let width = 760f32.min(size.width.as_f32() - 24.);
        let navigation_width = if width < 620. { 140. } else { 192. };
        let height = 600f32.min(size.height.as_f32() - 32.);
        let path = dialog.path.clone();
        let section = dialog.section;
        let weak = cx.weak_entity();
        let body = match section {
            ProjectSection::Codebase => self.project_codebase_section(&path, cx),
            ProjectSection::Notifications => self.project_notifications_section(&path, cx),
            _ => dialog
                .body
                .clone()
                .map(|view| view.into_any_element())
                .unwrap_or_else(|| div().into_any_element()),
        };
        let nav = ProjectSection::ALL.map(|tab| {
            let label = tab.label();
            let selected = tab == section;
            let row = format!("project-settings-{}", label.to_lowercase());
            div()
                .id(SharedString::from(row.clone()))
                .debug_selector(move || row.clone())
                .role(Role::Tab)
                .aria_label(label)
                .aria_selected(selected)
                .w_full()
                .h(px(32.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(10.))
                .rounded(px(vorkspace_ui::radius::CONTROL))
                .text_size(px(13.))
                .line_height(px(20.))
                .text_color(content(if selected { 1. } else { 0.65 }))
                .when(selected, |d| d.bg(content(0.08)))
                .when(!selected, |d| {
                    d.hover(|d| {
                        d.bg(vorkspace_ui::fluid::hover(0.05))
                            .text_color(content(1.))
                    })
                })
                .child(icon(tab.icon(), 14.).text_color(content(if selected { 1. } else { 0.65 })))
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.show_project_section(tab, window, cx)
                }))
        });
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
                        "project-settings-modal",
                        Anchor::Point(gpui::point(
                            px((size.width.as_f32() - width) / 2.),
                            px((size.height.as_f32() - height) / 2.),
                        )),
                    )
                    .modal_surface(true)
                    .without_animation()
                    .priority(60)
                    .width(width)
                    .height(height)
                    .gap(0.)
                    .on_dismiss(move |window, cx| {
                        let _ = weak.update(cx, |this, cx| this.close_project_settings(window, cx));
                    })
                    .child(
                        div()
                            .id("project-settings-dialog")
                            .debug_selector(|| "project-settings-dialog".into())
                            .role(Role::Dialog)
                            .aria_label(format!("{} settings", self.project_label(&path)))
                            .track_focus(&dialog.focus)
                            .on_key_down(cx.listener(
                                |this, event: &gpui::KeyDownEvent, window, cx| {
                                    if event.keystroke.key == "escape" {
                                        this.close_project_settings(window, cx);
                                        cx.stop_propagation();
                                    }
                                },
                            ))
                            .size_full()
                            .flex()
                            .relative()
                            .child(
                                div()
                                    .id("project-settings-navigation")
                                    .role(Role::TabList)
                                    .aria_label("Project settings sections")
                                    .w(px(navigation_width))
                                    .flex_shrink_0()
                                    .h_full()
                                    .border_r_1()
                                    .border_color(content(0.04))
                                    .p_2()
                                    .flex()
                                    .flex_col()
                                    .gap_px()
                                    // The project itself heads the rail, as the
                                    // Bot's face heads its settings.
                                    .child(
                                        div()
                                            .px_2p5()
                                            .pt_1p5()
                                            .pb_3()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .min_w_0()
                                            .child(self.project_mark(&path, 18.))
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .truncate()
                                                    .text_size(px(13.))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .child(self.project_label(&path)),
                                            ),
                                    )
                                    .children(nav),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .h(px(48.))
                                            .flex_shrink_0()
                                            .flex()
                                            .items_center()
                                            .px_5()
                                            .pr(px(48.))
                                            .border_b_1()
                                            .border_color(content(0.04))
                                            .text_size(px(14.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(section.label()),
                                    )
                                    .child(
                                        div()
                                            .id("project-settings-body")
                                            .debug_selector(|| "project-settings-body".into())
                                            .flex_1()
                                            .min_h_0()
                                            .overflow_y_scroll()
                                            // The background editor pads itself.
                                            .when(section != ProjectSection::Background, |d| {
                                                d.px_5().py_4()
                                            })
                                            .child(div().max_w(px(480.)).child(body)),
                                    ),
                            )
                            .child(
                                div().absolute().right_3().top(px(10.)).child(
                                    icon_button(
                                        "project-settings-close",
                                        "Close project settings",
                                        "close",
                                    )
                                    .debug_selector(|| "project-settings-close".into())
                                    .size(px(28.))
                                    .on_click(cx.listener(
                                        |this, _, window, cx| {
                                            this.close_project_settings(window, cx)
                                        },
                                    )),
                                ),
                            ),
                    ),
                )
                .into_any_element(),
        )
    }

    fn project_codebase_section(&self, path: &str, cx: &mut Context<Self>) -> AnyElement {
        let remote = vorkspace_core::files::remote::is_remote(std::path::Path::new(path));
        let on = crate::codebase::enabled(path, cx);
        let target = path.to_owned();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_4()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .line_height(px(20.))
                                    .child("Codebase Memory"),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .line_height(px(18.))
                                    .text_color(content(0.5))
                                    .child(if remote {
                                        "Not available for remote projects."
                                    } else {
                                        "A local index of this project's symbols and calls, for you and its agents. The first use downloads a 48 MB engine."
                                    }),
                            ),
                    )
                    .child(
                        crate::controls::switch(
                            "project-codebase-switch",
                            "Codebase Memory",
                            on,
                            !remote,
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if !remote {
                                this.configure_codebase(target.clone(), window, cx)
                            }
                        })),
                    ),
            )
            .into_any_element()
    }

    fn project_notifications_section(&self, path: &str, cx: &mut Context<Self>) -> AnyElement {
        let now = vorkspace_core::time::now_ms();
        let prefs = &self.workspace.preferences;
        let until = prefs.known_notification_project_id(path).and_then(|id| {
            let preference = &prefs.project_notifications[id];
            if preference.get("mutedUntil") == Some(&serde_json::Value::Null) {
                Some(None)
            } else {
                preference["mutedUntil"]
                    .as_u64()
                    .filter(|at| *at > now)
                    .map(Some)
            }
        });
        let status = match until {
            None => "Notifications are on for this project.".to_owned(),
            Some(None) => "Muted until you resume them.".to_owned(),
            Some(Some(at)) => {
                let minutes = at.saturating_sub(now).div_ceil(60_000);
                if minutes >= 60 {
                    format!("Muted for about {} more hours.", minutes.div_ceil(60))
                } else {
                    format!("Muted for {minutes} more minutes.")
                }
            }
        };
        let action = |id: &'static str, label: &'static str, cx: &mut Context<Self>| {
            let target = path.to_owned();
            crate::controls::secondary_button(
                SharedString::from(format!("project-notifications-{id}")),
                label,
                false,
                false,
            )
            .child(label)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.run_project_notification_menu(&target, id, window, cx)
            }))
        };
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_size(px(13.))
                    .line_height(px(20.))
                    .text_color(content(0.8))
                    .child(status),
            )
            .child(div().flex().flex_wrap().gap_2().map(|row| {
                if until.is_some() {
                    row.child(action("mute:resume", "Resume notifications", cx))
                } else {
                    row.items_center()
                        .child(
                            div()
                                .mr_1()
                                .text_size(px(12.))
                                .text_color(content(0.5))
                                .child("Mute for"),
                        )
                        .child(action("mute:1h", "1 hour", cx))
                        .child(action("mute:4h", "4 hours", cx))
                        .child(action("mute:8h", "8 hours", cx))
                        .child(action("mute:forever", "Until resumed", cx))
                }
            }))
            .child(
                div()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(content(0.45))
                    .child("Which events notify you is set for every project in Settings → Plugins."),
            )
            .into_any_element()
    }

    pub(super) fn project_settings_flyout(
        &self,
        events: &mut Vec<Subscription>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Flyout {
        let target = MenuTarget::Project {
            path: self.workspace.project.clone(),
            rail: false,
        };
        let (label, items, width, style) = self.menu_items(&target, events, window, cx);
        let menu = cx.new(|cx| Menu::embedded(label, items, cx).width(width).style(style));
        let surface = menu.read(cx).surface_region();
        let focus = menu.read(cx).content_focus();
        events.push(cx.subscribe_in(
            &menu,
            window,
            move |this, _, event: &MenuEvent, window, cx| {
                this.close_menu(cx);
                if let MenuEvent::Picked(action, row) = event {
                    this.run_menu(target.clone(), action, *row, window, cx);
                }
                if this.background_dialog.is_none() {
                    this.focus_workspace(window, cx);
                }
                cx.notify();
            },
        ));
        Flyout {
            view: menu.into(),
            width,
            padding: 8.,
            height: None,
            max_height: Some(640.),
            focus: Some(focus),
            surface: Some(surface),
        }
    }
    fn close_background_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.background_dialog = None;
        if let Some(bots) = &self.bots {
            bots.update(cx, |bots, cx| bots.close_background(cx));
        }
        self.focus_workspace(window, cx);
        cx.notify();
    }
    pub(super) fn background_modal(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let view = self
            .background_dialog
            .as_ref()
            .map(|open| open.view.clone())
            .or_else(|| {
                self.bots
                    .as_ref()
                    .and_then(|bots| bots.read(cx).background_dialog())
            })?;
        let viewport = window.viewport_size();
        let width = 420f32.min(viewport.width.as_f32() - 24.);
        let height = (view.read(cx).measured_height().unwrap_or(622.) + 2.)
            .min(viewport.height.as_f32() - 32.);
        let popup_region = view.read(cx).popup_region();
        let anchor = Anchor::Point(gpui::point(
            px((viewport.width.as_f32() - width) / 2.),
            px((viewport.height.as_f32() - height) / 2.),
        ));
        let weak = cx.weak_entity();
        Some(
            div()
                .absolute()
                .size_full()
                .bg(vorkspace_ui::fluid::scrim(0.4))
                .child(
                    popover("project-background-modal", anchor)
                        .modal_surface(self.workspace.classic_layout)
                        .width(width)
                        .gap(0.)
                        .max_height(viewport.height.as_f32() - 32.)
                        .ignore(popup_region)
                        .priority(60)
                        .on_dismiss(move |window, cx| {
                            let _ = weak
                                .update(cx, |this, cx| this.close_background_dialog(window, cx));
                        })
                        .child(view),
                )
                .into_any_element(),
        )
    }
    pub(super) fn chat_with_background(
        &mut self,
        id: i64,
        chat: Option<Entity<Conversation>>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let Some(chat) = chat else {
            return div().flex_1().min_h_0();
        };
        let (path, empty) = {
            let session = chat.read(cx);
            (session.summary.cwd.clone(), !session.summary.has_messages)
        };
        let mut appearance = self.appearance(&path);
        if appearance.background.is_none() {
            let p = &self.workspace.preferences;
            appearance.background = p.chat_background_settings();
        }
        let classic = self.workspace.classic_layout;
        chat.update(cx, |session, cx| {
            session.set_project_appearance(appearance.clone(), cx);
            session.set_classic_layout(classic, cx);
        });
        let settings = appearance.background.filter(|b| b.opacity(empty) > 0.);
        if settings.is_none() {
            self.backgrounds.remove(&id);
        }
        let background = settings.map(|settings| {
            let clip = (!self.workspace.classic_layout).then(|| self.background_card.clone());
            let background = self
                .backgrounds
                .entry(id)
                .or_insert_with(|| cx.new(|_| ProjectBackground::default()))
                .clone();
            background.update(cx, |background, cx| {
                background.configure(Some(settings), empty, clip, cx)
            });
            background
        });
        let linked = if self.workspace.active == Some(id) && !self.review_focused {
            self.linked_notice(id, cx)
        } else {
            None
        };
        div()
            .flex_1()
            .min_h_0()
            .relative()
            .overflow_hidden()
            .when_some(background, |d, background| {
                d.child(background.cached(StyleRefinement::default().absolute().size_full()))
            })
            .child(
                div()
                    .relative()
                    .size_full()
                    .child(chat.cached(StyleRefinement::default().size_full())),
            )
            .children(linked.map(|notice| {
                div()
                    .absolute()
                    .top(px(12.))
                    .right(px(12.))
                    .w(px(320.))
                    .max_w_full()
                    .child(notice)
            }))
    }
}

impl Shell {
    pub(super) fn project_notification_menu(&self, cwd: &str) -> MenuAction {
        let now = vorkspace_core::time::now_ms();
        let prefs = &self.workspace.preferences;
        let muted = prefs.known_notification_project_id(cwd).is_some_and(|id| {
            let preference = &prefs.project_notifications[id];
            preference.get("mutedUntil") == Some(&serde_json::Value::Null)
                || preference["mutedUntil"].as_u64().is_some_and(|at| at > now)
        });
        if muted {
            MenuAction::new("mute:resume", "Resume notifications").icon("bell")
        } else {
            MenuAction::new("notifications-mute", "Mute notifications")
                .icon("bell-off")
                .submenu(vec![
                    MenuAction::new("mute:1h", "For 1 hour"),
                    MenuAction::new("mute:4h", "For 4 hours"),
                    MenuAction::new("mute:8h", "For 8 hours"),
                    MenuAction::new("mute:forever", "Until resumed"),
                    MenuAction::new("mute:custom", "Custom…"),
                ])
        }
    }

    pub(super) fn run_project_notification_menu(
        &mut self,
        cwd: &str,
        action: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if action == "mute:custom" {
            self.open_settings(crate::settings::Section::Connectors, window, cx);
            return;
        }
        if action == "mute:resume" {
            self.workspace.preferences.resume_notification_project(cwd);
        } else {
            let hours = match action {
                "mute:1h" => Some(1),
                "mute:4h" => Some(4),
                "mute:8h" => Some(8),
                "mute:forever" => None,
                _ => return,
            };
            let until = hours.map(|hours| vorkspace_core::time::now_ms() + hours * 3_600_000);
            self.workspace
                .preferences
                .mute_notification_project(cwd, until);
        }
        self.apply_preferences(cx);
        self.sync_settings(cx);
        self.persist(cx);
        cx.notify();
    }
}
