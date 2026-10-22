use crate::settings::SettingsView;
use crate::{
    settings::{SettingsEvent, management::Target, widgets::icon_action},
    theme::*,
};
use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};
impl SettingsView {
    pub(super) fn bots_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let p = self.prefs.clone();
        let mut rows=vec![
            self.row(Some("bots-enabled"),true,"Show bots",Some("Agents of your own on the project rail. Each works on the projects you give it, remembers what matters and picks up habits it runs on its own. Turn this off to hide them.".into()),vec![self.toggle("bots-enabled","Show bots",p.bots,false,cx,|this,on,cx|this.update(cx,|p|p.bots=on))]),
            self.row(Some("rail-bots-pinned"),false,"Pin bots to the icon rail",Some("When the project rail is collapsed to icons, show each Bot at the top of the rail, above a divider, instead of inside the project picker.".into()),vec![self.toggle("rail-bots-pinned","Pin bots to the icon rail",p.rail_bots_pinned,!p.bots,cx,|this,on,cx|this.update(cx,|p|p.rail_bots_pinned=on))])];
        if cfg!(target_os = "macos") {
            rows.push(self.row(Some("bot-menu-bar-icon"),false,"Menu bar icon",Some("Chat with a Bot or open the quick composer from the macOS menu bar. Turn this off to hide the icon.".into()),vec![self.toggle("bot-menu-bar-icon","Menu bar icon",p.bot_menu_bar_icon,false,cx,|this,on,cx|this.update(cx,|p|p.bot_menu_bar_icon=on))]));
        }
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
                        .child(crate::bots::avatar::avatar(
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
                    .child("Show Bot spawned session on the sidebar")
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
        vec![self.group(None,true,"Bots",None,None,rows),self.group(Some("bot-list"),false,"Your bots",Some("Show or hide sessions started by each Bot. Hidden sessions remain available from the Bot's chat.".into()),None,roster)]
    }
}
