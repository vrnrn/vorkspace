use crate::settings::SettingsView;
use crate::settings::{SettingsEvent, management::Target, widgets::icon_action};
use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};
use vorkspace_ui::theme::*;
impl SettingsView {
    pub(super) fn bots_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let p = self.prefs.clone();
        let rows = vec![
            self.row(
                Some("bots-enabled"),
                true,
                "Show in navigation",
                None,
                vec![self.toggle(
                    "bots-enabled",
                    "Show bots",
                    p.bots,
                    false,
                    cx,
                    |this, on, cx| this.update(cx, |p| p.bots = on),
                )],
            ),
            self.row(
                Some("rail-bots-pinned"),
                false,
                "Pin bots to the icon rail",
                Some("Keep Bots above projects when navigation is collapsed.".into()),
                vec![self.toggle(
                    "rail-bots-pinned",
                    "Pin bots to the icon rail",
                    p.rail_bots_pinned,
                    !p.bots,
                    cx,
                    |this, on, cx| this.update(cx, |p| p.rail_bots_pinned = on),
                )],
            ),
            self.row(
                Some("bot-channels"),
                false,
                div().flex().items_center().gap_2().child("Channels").child(
                    vorkspace_ui::fluid::badge("Experimental", vorkspace_ui::theme::warning()),
                ),
                Some("Shared conversations with your Bots.".into()),
                vec![self.toggle(
                    "bot-channels",
                    "Channels",
                    p.channels,
                    !p.bots,
                    cx,
                    |this, on, cx| this.update(cx, |p| p.channels = on),
                )],
            ),
            self.row(
                Some("bot-minions"),
                false,
                div().flex().items_center().gap_2().child("Minions").child(
                    vorkspace_ui::fluid::badge("Experimental", vorkspace_ui::theme::warning()),
                ),
                Some("Let Bots delegate work to Minions that share their memory.".into()),
                vec![self.toggle(
                    "bot-minions",
                    "Minions",
                    p.minions,
                    !p.bots,
                    cx,
                    |this, on, cx| this.update(cx, |p| p.minions = on),
                )],
            ),
        ];
        let mut roster = Vec::new();
        for (ix, bot) in self.ctx.bots.clone().into_iter().enumerate() {
            let show_id = bot.id.clone();
            let show = bot.show;
            let folders = bot.folders;
            let description = match bot.projects.split_last() {
                None => "No projects yet".to_owned(),
                Some((last, [])) => format!("Works on {last}"),
                Some((last, others)) => format!("Works on {} and {last}", others.join(", ")),
            };
            let label = div()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .debug_selector(|| "settings-bot-mascot".into())
                        .size(px(16.))
                        .flex_shrink_0()
                        .child(crate::avatar::avatar(
                            &bot.mascot,
                            bot.color,
                            "idle",
                            16.,
                            true,
                        )),
                )
                .child(
                    div()
                        .debug_selector(|| "settings-bot-name".into())
                        .min_w_0()
                        .truncate()
                        .child(bot.name.clone()),
                );
            let reset = bot.clone();
            let controls = vec![
                div()
                    .text_size(px(12.))
                    .line_height(px(20.))
                    .text_color(content(0.5))
                    .child("Show sessions in sidebar")
                    .into_any_element(),
                div()
                    .debug_selector(|| "settings-bot-sidebar-switch".into())
                    .flex_shrink_0()
                    .child(self.toggle(
                        "bot-sidebar-show",
                        format!("Show sessions started by {} in sidebar", bot.name),
                        show,
                        false,
                        cx,
                        move |_, on, cx| {
                            cx.emit(SettingsEvent::BotSidebar(show_id.clone(), on, folders))
                        },
                    ))
                    .into_any_element(),
                icon_action(
                    "reset-bot",
                    format!("Reset {} to defaults", bot.name),
                    "rotate-ccw",
                    28.,
                    14.,
                    0.4,
                    false,
                )
                .debug_selector(|| "settings-bot-reset".into())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_management(Target::ResetBot(reset.clone()), window, cx)
                }))
                .into_any_element(),
            ];
            roster.push(
                div()
                    .id(SharedString::from(format!("settings-bot-{}", bot.id)))
                    .child(self.row(None, ix == 0, label, Some(description.into()), controls))
                    .into_any_element(),
            );
        }
        if roster.is_empty() {
            roster.push(super::super::widgets::note("No bots yet."));
        }
        vec![self.card(None, rows).into_any_element(),self.group(Some("bot-list"),false,"Your bots",Some("Show or hide sessions started by each Bot. Hidden sessions remain available from the Bot's chat.".into()),None,roster)]
    }
}
