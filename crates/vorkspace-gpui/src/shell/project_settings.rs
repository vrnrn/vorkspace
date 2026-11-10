use super::*;
use crate::settings::ProjectNotifications;
use crate::{
    background_settings::{BackgroundDialog, Event, Owner},
    project_background::ProjectBackground,
    project_mark::project_icon,
};

pub(super) struct OpenBackground {
    pub(super) view: Entity<BackgroundDialog>,
    /// The vorkspace whose background this edits; its dialog reopens after.
    vorkspace: String,
    _events: Subscription,
}
impl Shell {
    /// Edits a vorkspace's chat background in the background editor, in
    /// place of the vorkspace dialog, which comes back when it closes.
    pub(super) fn open_vorkspace_background(
        &mut self,
        vorkspace: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(def) = self.workspace.vorkspaces.iter().find(|v| v.id == vorkspace) else {
            return;
        };
        let owner = Owner::Vorkspace {
            id: vorkspace.clone(),
            label: def.name.clone(),
        };
        let view = cx
            .new(|cx| BackgroundDialog::new(owner, def.background.clone(), self.store.clone(), cx));
        let events = cx.subscribe_in(&view, window, {
            let vorkspace = vorkspace.clone();
            move |this, _, event: &Event, window, cx| match event {
                Event::Changed(background) => {
                    if let Some(v) = this.workspace.vorkspace_mut(&vorkspace) {
                        v.background = background.clone();
                    }
                    this.backgrounds.clear();
                    this.invalidate_chrome(cx);
                    this.persist(cx);
                    cx.notify();
                }
                Event::Close => this.close_background_dialog(window, cx),
            }
        });
        let focus = view.focus_handle(cx);
        self.vorkspace_dialog = None;
        self.background_dialog = Some(OpenBackground {
            view,
            vorkspace,
            _events: events,
        });
        window.focus(&focus, cx);
        cx.notify();
    }
    pub(super) fn project_label(&self, path: &str) -> String {
        crate::workspace::project_name(path)
    }
    pub(super) fn project_mark(&self, path: &str, side: f32) -> AnyElement {
        project_icon(path, side)
    }
    /// A project's settings live in its vorkspace's dialog, on its own tab.
    pub(super) fn open_project_settings(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_menu(cx);
        self.close_preview(cx);
        let key = vorkspace_core::files::path_key(&path);
        let vorkspace = self
            .workspace
            .active_vorkspace()
            .filter(|v| {
                v.projects
                    .iter()
                    .any(|p| vorkspace_core::files::path_key(p) == key)
            })
            .or_else(|| {
                self.workspace.vorkspaces.iter().find(|v| {
                    v.projects
                        .iter()
                        .any(|p| vorkspace_core::files::path_key(p) == key)
                })
            })
            .map(|v| v.id.clone());
        self.open_vorkspace_dialog(vorkspace, window, cx);
        if let Some(dialog) = self.vorkspace_dialog.as_mut() {
            dialog.project = Some(path);
        }
        cx.notify();
    }

    pub(super) fn project_codebase_section(
        &self,
        path: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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

    pub(super) fn project_notifications_section(
        &self,
        path: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
                    .child(
                        "Which events notify you is set for every project in Settings → Plugins.",
                    ),
            )
            .into_any_element()
    }

    pub(super) fn close_background_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let vorkspace = self.background_dialog.take().map(|open| open.vorkspace);
        if let Some(bots) = &self.bots {
            bots.update(cx, |bots, cx| bots.close_background(cx));
        }
        match vorkspace {
            Some(id) => self.open_vorkspace_dialog(Some(id), window, cx),
            None => self.focus_workspace(window, cx),
        }
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
                        .modal_surface()
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
            if !self.session_available_here(id, cx) {
                return crate::bots::conversation_in_other_window(cx.listener(
                    move |shell, _: &gpui::ClickEvent, window, cx| shell.open(id, window, cx),
                ));
            }
            return div().flex_1().min_h_0();
        };
        let empty = !chat.read(cx).summary.has_messages;
        let settings = self
            .workspace
            .active_vorkspace()
            .and_then(|v| v.background.clone())
            .or_else(|| self.workspace.preferences.chat_background_settings())
            .filter(|background| background.opacity(empty) > 0.);
        chat.update(cx, |session, cx| session.set_classic_layout(true, cx));
        if settings.is_none() {
            self.backgrounds.remove(&id);
        }
        let background = settings.map(|settings| {
            let background = self
                .backgrounds
                .entry(id)
                .or_insert_with(|| cx.new(|_| ProjectBackground::default()))
                .clone();
            background.update(cx, |background, cx| {
                background.configure(Some(settings), empty, None, cx)
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
