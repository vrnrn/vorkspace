//! The Bot settings dialog and the pickers it opens.
use super::*;
use vorkspace_core::bot::CompactionMode;
use vorkspace_ui::radius;

fn compaction_label(mode: CompactionMode) -> &'static str {
    match mode {
        CompactionMode::SessionRollover => "Fresh session",
        CompactionMode::Native => "Native compaction",
    }
}

impl Bots {
    pub(crate) fn settings_modal(
        &mut self,
        bot: &Bot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // Background editing temporarily owns the modal surface. Retain the
        // settings section and draft, but don't paint an occluding parent above
        // that editor (or let its outside-click handler dismiss the parent).
        if bot.is_minion() || self.background.is_some() {
            return None;
        }

        if self
            .creation
            .as_ref()
            .is_some_and(|draft| draft.bot.id != bot.id)
        {
            return None;
        }
        let creating = self
            .creation
            .as_ref()
            .is_some_and(|draft| draft.bot.id == bot.id);
        let section = if creating {
            self.settings.unwrap_or(SettingsSection::General)
        } else {
            self.settings?
        };
        let size = window.viewport_size();
        let width = 880f32.min(size.width.as_f32() - 24.);
        let navigation_width = if width < 660. { 140. } else { 192. };
        let height = 680f32.min(size.height.as_f32() - 32.);
        let delete_anchor = AnchorSlot::default();
        let body = match section {
            SettingsSection::General => {
                self.general_settings(bot, delete_anchor.clone(), window, cx)
            }
            SettingsSection::Soul => self.soul_page(bot, cx),
            SettingsSection::Memory => self.memory_page(bot, window, cx),
            SettingsSection::Habits => self.habit_panel.clone().into_any_element(),
        };
        self.bind_settings_blurs(window, cx);
        let weak = cx.weak_entity();
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
                    .with_priority(69),
                )
                .child(
                    popover(
                        "bot-settings-modal",
                        Anchor::Point(gpui::point(
                            px((size.width.as_f32() - width) / 2.),
                            px((size.height.as_f32() - height) / 2.),
                        )),
                    )
                    .ignore(self.notice_region.clone())
                    .when_some(self.picker.as_ref(), |popover, picker| {
                        // Pickers are deferred above the dialog and can extend past
                        // its bounds. Their surfaces still belong to this setup.
                        let menu = picker.menu.read(cx);
                        popover
                            .ignore(menu.surface_region())
                            .ignore(menu.submenu_region())
                    })
                    .modal_surface()
                    .without_animation()
                    .priority(70)
                    .width(width)
                    .height(height)
                    .gap(0.)
                    .on_dismiss(move |window, cx| {
                        let _ = weak.update(cx, |this, cx| {
                            if creating {
                                this.cancel_creation(window, cx);
                                return;
                            }
                            this.save_open_file(cx);
                            this.settings = None;
                            this.picker = None;
                            cx.notify();
                        });
                    })
                    .child(
                        div()
                            .id("bot-settings-dialog")
                            .debug_selector(|| "bot-settings-dialog".into())
                            .role(Role::Dialog)
                            .track_focus(&self.settings_focus)
                            .aria_label(if creating {
                                "New bot".into()
                            } else {
                                format!("{} settings", bot.label())
                            })
                            .h_full()
                            .w_full()
                            .flex()
                            .relative()
                            .child(
                                div()
                                    .id("bot-settings-navigation")
                                    .debug_selector(|| "bot-settings-navigation".into())
                                    .role(Role::Navigation)
                                    .aria_label("Settings sections")
                                    .w(px(navigation_width))
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
                                            .px_2p5()
                                            .pt_1p5()
                                            .pb_2()
                                            .text_size(px(11.))
                                            .text_color(content(0.45))
                                            .child(if creating { "New bot" } else { "Settings" }),
                                    )
                                    .children(
                                        [
                                            (SettingsSection::General, "General", "settings"),
                                            (SettingsSection::Soul, "Soul", "sparkles"),
                                            (SettingsSection::Memory, "Memory", "sticky-note"),
                                            (SettingsSection::Habits, "Habits", "clock"),
                                        ]
                                        .into_iter()
                                        .filter(|(tab, _, _)| {
                                            !creating
                                                || matches!(
                                                    tab,
                                                    SettingsSection::General
                                                        | SettingsSection::Soul
                                                )
                                        })
                                        .map(
                                            |(tab, label, glyph)| {
                                                let row_id = format!("bot-settings-{label}");
                                                div()
                                                    .id(SharedString::from(row_id.clone()))
                                                    .debug_selector(move || row_id.clone())
                                                    .role(Role::Button)
                                                    .aria_label(label)
                                                    .w_full()
                                                    .h(px(32.))
                                                    .flex_shrink_0()
                                                    .flex()
                                                    .flex_row()
                                                    .items_center()
                                                    .justify_start()
                                                    .text_align(gpui::TextAlign::Left)
                                                    .gap(px(10.))
                                                    .px(px(10.))
                                                    .py(px(6.))
                                                    .rounded(px(radius::CONTROL))
                                                    .text_size(px(13.))
                                                    .line_height(px(20.))
                                                    .text_color(content(if section == tab {
                                                        1.
                                                    } else {
                                                        0.65
                                                    }))
                                                    .when(section == tab, |d| d.bg(content(0.08)))
                                                    .when(section != tab, |d| {
                                                        d.hover(|d| {
                                                            d.bg(content(0.05))
                                                                .text_color(content(1.))
                                                        })
                                                    })
                                                    .child(
                                                        div()
                                                            .id(SharedString::from(format!(
                                                                "bot-settings-icon-{label}"
                                                            )))
                                                            .debug_selector(move || {
                                                                format!("bot-settings-icon-{label}")
                                                            })
                                                            .size(px(14.))
                                                            .flex_shrink_0()
                                                            .child(icon(glyph, 14.).text_color(
                                                                content(if section == tab {
                                                                    1.
                                                                } else {
                                                                    0.65
                                                                }),
                                                            )),
                                                    )
                                                    .child(
                                                        div()
                                                            .id(SharedString::from(format!(
                                                                "bot-settings-label-{label}"
                                                            )))
                                                            .debug_selector(move || {
                                                                format!(
                                                                    "bot-settings-label-{label}"
                                                                )
                                                            })
                                                            .child(label),
                                                    )
                                                    .on_click(cx.listener(
                                                        move |this, _, window, cx| {
                                                            this.save_open_file(cx);
                                                            this.settings = Some(tab);
                                                            if tab == SettingsSection::Memory
                                                                && let Some(id) =
                                                                    this.active.clone()
                                                            {
                                                                this.reload_files(&id, cx);
                                                            }
                                                            window.focus(&this.settings_focus, cx);
                                                            cx.notify();
                                                        },
                                                    ))
                                            },
                                        ),
                                    )
                                    .when(!creating, |d| {
                                        let id = bot.id.clone();
                                        d.child(div().flex_1()).child(
                                            div()
                                                .id("bot-delete")
                                                .debug_selector(|| "bot-delete".into())
                                                .role(Role::Button)
                                                .map(vorkspace_ui::theme::tab_reachable)
                                                .aria_label("Delete bot")
                                                .relative()
                                                .w_full()
                                                .h(px(32.))
                                                .flex_shrink_0()
                                                .flex()
                                                .items_center()
                                                .gap(px(10.))
                                                .px(px(10.))
                                                .rounded(px(radius::CONTROL))
                                                .text_size(px(13.))
                                                .line_height(px(20.))
                                                .text_color(vorkspace_ui::theme::danger())
                                                .hover(|d| {
                                                    d.bg(vorkspace_ui::fluid::wash(
                                                        vorkspace_ui::theme::danger().opacity(0.08),
                                                    ))
                                                })
                                                .child(delete_anchor.probe())
                                                .child(
                                                    icon("trash", 14.)
                                                        .text_color(vorkspace_ui::theme::danger()),
                                                )
                                                .child("Delete bot…")
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.save_open_file(cx);
                                                        this.delete_bot(&id, window, cx);
                                                    },
                                                )),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .flex()
                                    .flex_col()
                                    .child(body)
                                    .when(creating || section == SettingsSection::General, |d| {
                                        d.child(self.settings_footer(bot, creating, cx))
                                    }),
                            )
                            .child(
                                div().absolute().right_3().top_1p5().child(
                                    icon_button(
                                        "bot-settings-close",
                                        "Close Bot settings",
                                        "close",
                                    )
                                    .debug_selector(|| "bot-settings-close".into())
                                    .size(px(28.))
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            if creating {
                                                this.cancel_creation(window, cx);
                                                return;
                                            }
                                            this.save_open_file(cx);
                                            this.settings = None;
                                            cx.notify();
                                        },
                                    )),
                                ),
                            ),
                    ),
                )
                .into_any_element(),
        )
    }
    pub(super) fn settings_footer(
        &self,
        bot: &Bot,
        creating: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let enabled = !bot.name.trim().is_empty();
        let reset = bot.id.clone();
        div()
            .id("bot-settings-footer")
            .debug_selector(|| "bot-settings-footer".into())
            .flex_shrink_0()
            .min_h(px(56.))
            .px_6()
            .py_3()
            .border_t_1()
            .border_color(content(0.08))
            .flex()
            .items_center()
            .justify_end()
            .gap_2()
            .when(creating, |d| {
                d.child(
                    button("bot-create-cancel", "Cancel").on_click(
                        cx.listener(|this, _, window, cx| this.cancel_creation(window, cx)),
                    ),
                )
                .child(
                    div()
                        .id("bot-create")
                        .debug_selector(|| "bot-create".into())
                        .role(Role::Button)
                        .map(vorkspace_ui::theme::tab_reachable)
                        .aria_label("Create")
                        .aria_description(if enabled {
                            "Create this named Bot"
                        } else {
                            "Enter a Bot name to enable Create"
                        })
                        .h(px(30.))
                        .min_w(px(76.))
                        .px_4()
                        .rounded(px(radius::CONTROL))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .bg(rgb(0x0a84ff).opacity(if enabled { 1. } else { 0.28 }))
                        .text_color(if enabled { black(1.) } else { content(0.45) })
                        .when(enabled, |d| d.hover(|d| d.bg(rgb(0x409cff))))
                        .child("Create")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if enabled {
                                this.confirm_creation(window, cx);
                            }
                        })),
                )
            })
            .when(!creating, |d| {
                d.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.))
                        .text_color(content(0.55))
                        .child("Reset messages; keep soul, memory and habits."),
                )
                .child(
                    button("bot-reset", "Reset")
                        .debug_selector(|| "bot-reset".into())
                        .text_color(vorkspace_ui::theme::danger())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.reset_conversation(&reset, window, cx)
                        })),
                )
            })
            .into_any_element()
    }
    pub(super) fn general_settings(
        &mut self,
        bot: &Bot,
        delete_anchor: AnchorSlot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let creating = self
            .creation
            .as_ref()
            .is_some_and(|draft| draft.bot.id == bot.id);
        let narrow = window.viewport_size().width < px(780.);
        let name = self.field("bot-name", "Bot name", bot.name.clone(), false, cx);
        name.update(cx, |f, cx| {
            f.set_font_size(15.);
            f.set_line_height(20.);
            f.set_centered();
            f.set_placeholder(
                if creating {
                    "Name your bot".into()
                } else {
                    bot.label()
                },
                cx,
            );
        });
        let look = bot.id.clone();
        let add = bot.id.clone();
        let chat = self.chat();
        let (model, provider, mode, model_options) = if let Some(draft) = &self.creation {
            let model = draft
                .model()
                .map(|model| {
                    crate::pickers::model_display_name(draft.provider, &model.id, &model.name)
                })
                .unwrap_or_else(|| {
                    crate::pickers::model_name(draft.provider, draft.settings.model.as_deref())
                });
            (
                model,
                draft.provider.id().to_owned(),
                draft.settings.access,
                crate::conversation::bot_settings::model_options(
                    draft.provider,
                    draft.model(),
                    &draft.settings,
                ),
            )
        } else {
            (
                chat.as_ref()
                    .map(|view| view.read(cx).bot_model_label())
                    .unwrap_or_else(|| "Loading…".into()),
                chat.as_ref()
                    .map(|view| view.read(cx).summary.provider.clone())
                    .unwrap_or_else(|| "codex".into()),
                chat.as_ref()
                    .map(|view| view.read(cx).settings.access)
                    .unwrap_or(AccessMode::Auto),
                chat.as_ref()
                    .map(|view| view.read(cx).bot_model_options())
                    .unwrap_or_default(),
            )
        };
        let access_icon = match mode {
            AccessMode::FullAccess => "shield-alert",
            AccessMode::Auto => "sparkles",
            AccessMode::AutoAcceptEdits => "pencil-edit",
            _ => "square-lock",
        };
        let plain = |id, label: SharedString| {
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(label.clone())
                .h(px(28.))
                .px_1p5()
                .mx(px(-6.))
                .rounded(px(radius::CONTROL))
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(12.))
                .text_color(content(0.85))
                .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.06)))
                .child(label)
        };
        let identity = div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .w(px(160.))
            .flex_shrink_0()
            .child(
                div()
                    .id("bot-look")
                    .debug_selector(|| "bot-look".into())
                    .role(Role::Button)
                    .aria_label("Edit Bot appearance")
                    .tab_stop(true)
                    .relative()
                    .size(px(144.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_2xl()
                    .border_1()
                    .border_color(content(0.04))
                    .bg(content(0.04))
                    .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.08)))
                    .child(self.settings_anchor.probe())
                    .child(avatar::settings_preview(
                        bot,
                        128.,
                        delete_anchor,
                        window,
                        cx,
                    ))
                    .child(
                        div()
                            .debug_selector(|| "bot-look-edit".into())
                            .absolute()
                            .top_1()
                            .right_1()
                            .size(px(24.))
                            .rounded(px(radius::CONTROL))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(content(0.65))
                            .child(icon("pencil-edit", 14.)),
                    )
                    .on_click(
                        cx.listener(move |this, _, w, cx| this.open_settings_look(&look, w, cx)),
                    ),
            )
            .child(
                div()
                    .id("bot-name-field")
                    .debug_selector(|| "bot-name-field".into())
                    .mt_3()
                    .h(px(32.))
                    .w(px(160.))
                    .max_w_full()
                    .px_2()
                    .py_1p5()
                    .rounded(px(radius::CONTROL))
                    .font_weight(FontWeight::MEDIUM)
                    .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.05)))
                    .child(name),
            );
        let compaction_anchor = AnchorSlot::default();
        let compaction_target = bot.id.clone();
        let compaction_label = compaction_label(bot.compaction_mode);
        let model_controls = card()
            .id("bot-model-settings")
            .debug_selector(|| "bot-model-settings".into())
            .flex_1()
            .min_w_0()
            .child(setting(
                "Model",
                None,
                div()
                    .id("bot-settings-model")
                    .role(Role::Button)
                    .map(vorkspace_ui::theme::tab_reachable)
                    .aria_label(format!("Choose model, {model}"))
                    .h(px(28.))
                    .px_1p5()
                    .mx(px(-6.))
                    .rounded(px(radius::CONTROL))
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(content(0.85))
                    .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.06)))
                    .child(self.model_anchor.probe())
                    .child(provider_icon(&provider, 14.))
                    .child(div().min_w_0().truncate().child(model))
                    .on_click(cx.listener(|this, _, w, cx| this.open_model_picker(w, cx)))
                    .into_any_element(),
            ))
            .children(model_options.into_iter().filter_map(|item| {
                let MenuItem::Action(option) = item else {
                    return None;
                };
                let label = option.label.clone();
                let value = option
                    .value
                    .clone()
                    .unwrap_or_else(|| if option.checked { "On" } else { "Off" }.into());
                let anchor = AnchorSlot::default();
                let probe = anchor.probe();
                Some(
                    setting(
                        &label,
                        None,
                        plain(
                            SharedString::from(format!("bot-option-{}", option.id)),
                            value,
                        )
                        .child(probe)
                        .on_click(cx.listener(move |this, _, w, cx| {
                            if option.submenu.is_empty() {
                                if this.creating() {
                                    this.choose_creation_option(&option.id, cx);
                                } else if let Some(chat) = this.chat()
                                    && let Err(e) = chat
                                        .update(cx, |v, cx| v.bot_pick_model_option(&option.id, cx))
                                {
                                    this.error = Some(e);
                                }
                                this.changed(cx);
                            } else {
                                this.open_model_option(
                                    option.label.clone(),
                                    option.submenu.clone(),
                                    anchor.clone(),
                                    w,
                                    cx,
                                );
                            }
                        }))
                        .into_any_element(),
                    )
                    .border_t_1()
                    .border_color(content(0.04)),
                )
            }))
            .child(
                setting(
                    "Context compaction",
                    None,
                    plain("bot-settings-compaction".into(), compaction_label.into())
                        .debug_selector(|| "bot-settings-compaction".into())
                        .aria_label(format!("Context compaction, {compaction_label}"))
                        .child(compaction_anchor.probe())
                        .child(icon("chevron-down", 12.))
                        .on_click(cx.listener(move |this, _, w, cx| {
                            this.open_compaction_picker(
                                &compaction_target,
                                compaction_anchor.clone(),
                                w,
                                cx,
                            );
                        }))
                        .into_any_element(),
                )
                .border_t_1()
                .border_color(content(0.04)),
            )
            .child(
                setting(
                    "Permissions",
                    None,
                    div()
                        .id("bot-settings-access")
                        .role(Role::Button)
                        .map(vorkspace_ui::theme::tab_reachable)
                        .aria_label(format!("Permissions, {}", mode.label()))
                        .h(px(28.))
                        .max_w(px(208.))
                        .px_1p5()
                        .mx(px(-6.))
                        .rounded(px(radius::CONTROL))
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(px(12.))
                        .text_color(content(0.85))
                        .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.06)))
                        .child(self.access_anchor.probe())
                        .child(
                            icon(access_icon, 14.).when(mode == AccessMode::FullAccess, |i| {
                                i.text_color(vorkspace_ui::theme::warning())
                            }),
                        )
                        .child(mode.label())
                        .child(icon("chevron-down", 12.))
                        .on_click(cx.listener(|this, _, w, cx| this.open_access_picker(w, cx)))
                        .into_any_element(),
                )
                .border_t_1()
                .border_color(content(0.04)),
            );
        div()
            .id("bot-general-settings")
            .debug_selector(|| "bot-general-settings".into())
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .when(narrow, |d| d.px_4())
            .when(!narrow, |d| d.px_8())
            .pt_6()
            .pb_6()
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .child("General"),
            )
            .child(
                div()
                    .id("bot-settings-header")
                    .debug_selector(|| "bot-settings-header".into())
                    .mt_4()
                    .flex()
                    .gap_6()
                    .when(narrow, |d| d.flex_col())
                    .when(!narrow, |d| d.items_center())
                    .child(identity)
                    .child(model_controls),
            )
            .child(heading(
                "Projects",
                Some("It can see every session, note and folder in these."),
            ))
            .child(
                card()
                    .children(bot.projects.iter().enumerate().map(|(ix, project)| {
                        let remove = project.clone();
                        let owner = bot.id.clone();
                        let label = crate::workspace::project_name(project);
                        let pretty = std::env::var("HOME")
                            .ok()
                            .and_then(|home| {
                                project.strip_prefix(&home).map(|rest| format!("~{rest}"))
                            })
                            .unwrap_or_else(|| project.clone());
                        let group: SharedString = format!("bot-project-{ix}").into();
                        div()
                            .id(("bot-project", ix))
                            .group(group.clone())
                            .min_h(px(48.))
                            .flex()
                            .items_center()
                            .gap_3()
                            .px_3()
                            .py_2()
                            .when(ix > 0, |d| d.border_t_1().border_color(content(0.04)))
                            .child(
                                div()
                                    .size(px(32.))
                                    .flex_shrink_0()
                                    .rounded(px(radius::CONTAINER))
                                    .bg(content(0.05))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(crate::project_mark::project_icon(project, 16.)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .line_height(px(20.))
                                            .text_color(content(0.9))
                                            .truncate()
                                            .child(label),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .line_height(px(16.))
                                            .text_color(content(0.4))
                                            .truncate()
                                            .child(pretty),
                                    ),
                            )
                            .child(
                                icon_button("bot-remove-project", "Remove project", "close")
                                    .opacity(0.)
                                    .group_hover(group, |s| s.opacity(1.))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.edit_settings_bot(
                                            &owner,
                                            |bot| bot.projects.retain(|path| path != &remove),
                                            cx,
                                        );
                                    })),
                            )
                    }))
                    .child(
                        div()
                            .id("bot-add-project")
                            .role(Role::Button)
                            .aria_label("Add project")
                            .h(px(44.))
                            .px_3()
                            .flex()
                            .items_center()
                            .gap_3()
                            .text_size(px(13.))
                            .text_color(content(0.55))
                            .hover(|s| {
                                s.bg(vorkspace_ui::fluid::hover(0.04))
                                    .text_color(content(1.))
                            })
                            .when(!bot.projects.is_empty(), |d| {
                                d.border_t_1().border_color(content(0.04))
                            })
                            .child(self.projects_anchor.probe())
                            .child(
                                div()
                                    .size(px(32.))
                                    .rounded(px(radius::CONTAINER))
                                    .border_1()
                                    .border_color(content(0.15))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(icon("plus", 14.)),
                            )
                            .child("Add project")
                            .on_click(
                                cx.listener(move |this, _, w, cx| this.pick_project(&add, w, cx)),
                            ),
                    ),
            )
            .child(heading("Sessions it starts", None))
            .child(card().child(switch_setting(
                "Show sessions in sidebar",
                "Sessions it starts appear in the Sessions list for its projects.",
                self.switch(
                    "bot-show-started",
                    "Show sessions in sidebar",
                    bot.show_started_sessions_in_sidebar,
                    |m, on| m.show_started_sessions_in_sidebar = on,
                    cx,
                ),
            )))
            .into_any_element()
    }
    pub(super) fn switch(
        &self,
        id: &'static str,
        label: &'static str,
        on: bool,
        change: fn(&mut Bot, bool),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        crate::controls::switch(id, label, on, true)
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(id) = this
                    .creation
                    .as_ref()
                    .map(|draft| draft.bot.id.clone())
                    .or(this.active.clone())
                {
                    this.edit_settings_bot(&id, |bot| change(bot, !on), cx);
                }
            }))
            .into_any_element()
    }
    /// BotMenu, beside the row's options button or at a right-click.
    pub(crate) fn open_options(
        &mut self,
        id: &str,
        at: Option<gpui::Point<gpui::Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.minion_read_only(id) {
            return;
        }
        if self.roster.get(id).is_none() {
            return;
        }
        let menu = cx.new(|cx| {
            Menu::new(
                "Bot options",
                vec![
                    MenuAction::new("settings", "Bot settings")
                        .icon("settings")
                        .into(),
                ],
                window,
                cx,
            )
            .width(180.)
        });
        let target = id.to_string();
        let picked = cx.subscribe_in(&menu, window, move |this, _, event: &MenuEvent, w, cx| {
            this.menu = None;
            if let MenuEvent::Picked(action, _) = event
                && action.as_ref() == "settings"
            {
                this.open_bot_settings(&target, w, cx);
            }
            cx.notify();
        });
        self.menu_anchor = match at {
            Some(at) => Anchor::Point(at),
            None => self
                .rail_bounds
                .get(id)
                .cloned()
                .map(Anchor::Slot)
                .unwrap_or_else(|| Anchor::Slot(self.settings_anchor.clone())),
        };
        self.menu = Some((id.into(), menu, vec![picked]));
        cx.notify();
    }
    pub(crate) fn options_surface(&self, cx: &App) -> Option<AnyElement> {
        self.menu.as_ref().map(|(_, menu, _)| {
            menu_popover(menu, self.menu_anchor.clone(), cx)
                .without_animation()
                .side(Side::Right)
                .priority(90)
                .gap(0.)
                .into_any_element()
        })
    }
    pub(crate) fn picker_surface(&self, cx: &App) -> Option<AnyElement> {
        self.picker.as_ref().map(|p| {
            menu_popover(&p.menu, p.anchor.clone(), cx)
                .align(p.align)
                .without_animation()
                .priority(90)
                .width(p.width)
                .into_any_element()
        })
    }
    pub(super) fn open_model_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.creation.as_ref().map(|draft| draft.bot.id.clone());
        let (provider, model, project, account) = if let Some(draft) = &self.creation {
            (
                draft.provider,
                draft.settings.model.clone(),
                self.root.to_string_lossy().to_string(),
                draft.settings.account.clone(),
            )
        } else if let Some(chat) = self.chat() {
            let view = chat.read(cx);
            (
                Kind::from_id(&view.summary.provider),
                view.bot_model(),
                view.summary.cwd.clone(),
                view.settings.account.clone(),
            )
        } else {
            return;
        };
        let list = cx.new(|cx| ModelList::new_for(provider, model, &project, account, cx));
        let focus = list.read(cx).search_focus(cx);
        let picked = cx.subscribe(&list, move |this, _, picked: &ModelPicked, cx| {
            if let Some(target) = &target {
                let Some(draft) = this
                    .creation
                    .as_mut()
                    .filter(|draft| draft.bot.id == *target)
                else {
                    return;
                };
                draft.provider = picked.0;
                draft.settings.model = picked.1.clone();
                draft.settings.effort = crate::conversation::preferred_effort(picked.0.id(), cx);
                draft.settings.fast = false;
                draft.settings.thinking = None;
                draft.settings.service_tier = None;
                draft.settings.model_settings.clear();
                draft.catalog = Default::default();
                this.load_creation_catalog(cx);
            } else if let Some(view) = this.chat()
                && let Err(e) = view.update(cx, |v, cx| {
                    v.bot_config(picked.0, picked.1.clone(), HashMap::new(), cx)
                })
            {
                this.error = Some(e);
            }
            this.picker = None;
            this.changed(cx);
        });
        let menu = cx.new(|cx| {
            Menu::new(
                "Model",
                vec![MenuItem::Custom(list.into(), focus)],
                window,
                cx,
            )
            .width(400.)
        });
        let closed = cx.subscribe(&menu, |this, _, _: &MenuEvent, cx| {
            this.picker = None;
            cx.notify();
        });
        self.picker = Some(OpenPicker {
            menu,
            anchor: Anchor::Slot(self.model_anchor.clone()),
            width: 400.,
            align: crate::popover::Align::Start,
            _events: vec![picked, closed],
        });
        cx.notify();
    }
    pub(super) fn open_model_option(
        &mut self,
        label: SharedString,
        choices: Vec<MenuAction>,
        anchor: AnchorSlot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.settings_identity();
        let menu = cx.new(|cx| {
            Menu::new(
                label,
                choices.into_iter().map(Into::into).collect(),
                window,
                cx,
            )
        });
        let picked = cx.subscribe(&menu, move |this, _, event: &MenuEvent, cx| {
            if this.settings_identity() != target {
                return;
            }
            if let MenuEvent::Picked(id, _) = event {
                if this.creating() {
                    this.choose_creation_option(id, cx);
                } else if let Some(chat) = this.chat()
                    && let Err(e) = chat.update(cx, |v, cx| v.bot_pick_model_option(id, cx))
                {
                    this.error = Some(e);
                }
            }
            this.picker = None;
            this.changed(cx);
        });
        self.picker = Some(OpenPicker {
            menu,
            anchor: Anchor::Slot(anchor),
            width: 228.,
            align: crate::popover::Align::Start,
            _events: vec![picked],
        });
        cx.notify();
    }
    fn open_compaction_picker(
        &mut self,
        target: &str,
        anchor: AnchorSlot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings_identity().as_deref() != Some(target) {
            return;
        }
        let Some(current) = self.settings_bot(target).map(|bot| bot.compaction_mode) else {
            return;
        };
        let target = target.to_owned();
        let options = vec![
            MenuAction::new(
                "session-rollover",
                compaction_label(CompactionMode::SessionRollover),
            )
            .description("Start a fresh provider session at 80% with a brief from the chat.")
            .checked(current == CompactionMode::SessionRollover)
            .into(),
            MenuAction::new(
                "native-compaction",
                compaction_label(CompactionMode::Native),
            )
            .description("Keep the provider session and let the provider compact it.")
            .checked(current == CompactionMode::Native)
            .into(),
        ];
        let menu = cx.new(|cx| {
            Menu::new("Context compaction", options, window, cx)
                .style(MenuStyle::Access)
                .width(288.)
        });
        let picked = cx.subscribe(&menu, move |this, _, event: &MenuEvent, cx| {
            if this.settings_identity().as_deref() != Some(target.as_str()) {
                return;
            }
            this.picker = None;
            if let MenuEvent::Picked(id, _) = event {
                let mode = match id.as_ref() {
                    "session-rollover" => CompactionMode::SessionRollover,
                    "native-compaction" => CompactionMode::Native,
                    _ => return,
                };
                this.edit_settings_bot(&target, |bot| bot.compaction_mode = mode, cx);
            }
            cx.notify();
        });
        self.picker = Some(OpenPicker {
            menu,
            anchor: Anchor::Slot(anchor),
            width: 288.,
            align: crate::popover::Align::Start,
            _events: vec![picked],
        });
        cx.notify();
    }
    pub(super) fn open_access_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.settings_identity();
        let options = self
            .creation
            .as_ref()
            .map(|draft| {
                crate::conversation::bot_settings::access_options(
                    draft.provider,
                    draft.settings.access,
                    false,
                )
            })
            .or_else(|| self.chat().map(|chat| chat.read(cx).bot_access_options()))
            .unwrap_or_default();
        let menu = cx.new(|cx| {
            Menu::new("Permissions", options, window, cx)
                .style(MenuStyle::Access)
                .width(288.)
        });
        let picked = cx.subscribe(&menu, move |this, _, event: &MenuEvent, cx| {
            if this.settings_identity() != target {
                return;
            }
            if let MenuEvent::Picked(id, _) = event {
                let mode = AccessMode::from_id(id.strip_prefix("access:").unwrap_or(id));
                if let Some(draft) = &mut this.creation {
                    draft.settings.access = mode;
                } else if let Some(view) = this.chat() {
                    view.update(cx, |v, cx| v.bot_access(mode, cx));
                }
            }
            this.picker = None;
            cx.notify();
        });
        self.picker = Some(OpenPicker {
            menu,
            anchor: Anchor::Slot(self.access_anchor.clone()),
            width: 288.,
            align: crate::popover::Align::Start,
            _events: vec![picked],
        });
        cx.notify();
    }
    pub(super) fn pick_project(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let projects = self.owner.projects(cx);
        let bot = id.to_string();
        let paths = projects.clone();
        let mut items: Vec<MenuItem> = projects
            .into_iter()
            .enumerate()
            .map(|(ix, path)| {
                MenuAction::new(
                    format!("project:{ix}"),
                    crate::workspace::project_name(&path),
                )
                .icon("folder")
                .icon_color(rgb(vorkspace_core::project_appearance::project_color(&path)).into())
                .description(path)
                .into()
            })
            .collect();
        items.push(MenuItem::Separator);
        items.push(MenuAction::new("browse", "Open project…").into());
        let menu = cx.new(|cx| {
            Menu::new("Add project", items, window, cx)
                .style(MenuStyle::Composer)
                .width(300.)
        });
        let picked = cx.subscribe(&menu, move |this, _, event: &MenuEvent, cx| {
            this.picker = None;
            if let MenuEvent::Picked(action, _) = event {
                if action == "browse" {
                    this.browse_project(&bot, cx);
                } else if let Some(path) = action
                    .strip_prefix("project:")
                    .and_then(|n| n.parse::<usize>().ok())
                    .and_then(|ix| paths.get(ix))
                {
                    this.add_project(&bot, path.clone(), cx);
                }
            }
            cx.notify();
        });
        self.picker = Some(OpenPicker {
            menu,
            anchor: Anchor::Slot(self.projects_anchor.clone()),
            width: 300.,
            align: crate::popover::Align::Start,
            _events: vec![picked],
        });
        cx.notify();
    }
    pub(super) fn add_project(&mut self, id: &str, path: String, cx: &mut Context<Self>) {
        self.edit_settings_bot(id, |bot| bot.projects.push(path), cx);
    }
    pub(super) fn browse_project(&mut self, id: &str, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add project".into()),
        });
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |this, cx| {
                    this.add_project(&id, path.to_string_lossy().into(), cx)
                });
            }
        })
        .detach();
    }
    pub(crate) fn open_bot_settings(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.minion_read_only(id) {
            return;
        }
        if self.roster.get(id).is_none() {
            return;
        }
        self.open(id, window, cx);
        self.settings = Some(SettingsSection::General);
        window.focus(&self.settings_focus, cx);
        self.reload_files(id, cx);
        cx.notify();
    }
    pub(super) fn open_background(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(bot) = self.settings_bot(id).cloned() else {
            return;
        };
        let view = cx.new(|cx| {
            BackgroundDialog::new(
                crate::background_settings::Owner::Bot {
                    id: id.to_owned(),
                    label: bot.label(),
                },
                bot.background,
                self.store.clone(),
                cx,
            )
        });
        let id = id.to_string();
        let events = cx.subscribe_in(
            &view,
            window,
            move |this, _, event: &BackgroundEvent, window, cx| match event {
                BackgroundEvent::Changed(background) => {
                    this.edit_settings_bot(
                        &id,
                        |bot| {
                            bot.background = background.clone();
                        },
                        cx,
                    );
                }
                BackgroundEvent::Close => {
                    this.background = None;
                    window.focus(&this.settings_focus, cx);
                    this.changed(cx);
                }
            },
        );
        window.focus(&view.focus_handle(cx), cx);
        self.background = Some((view, events));
        self.changed(cx);
    }
    pub(crate) fn close_background(&mut self, cx: &mut Context<Self>) {
        if self.background.take().is_some() {
            self.changed(cx);
        }
    }
    pub(crate) fn background_dialog(&self) -> Option<Entity<BackgroundDialog>> {
        self.background.as_ref().map(|(view, _)| view.clone())
    }
}
