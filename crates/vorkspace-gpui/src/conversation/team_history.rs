//! Viewing a teammate is supervision, never user-message admission. The shared
//! classification survives settings reloads and does not depend on visibility,
//! the feature toggle, or whether a member is still employed.
use super::*;

impl Conversation {
    pub(crate) fn refresh_team_history(&mut self, cx: &mut Context<Self>) {
        if !self.user_read_only() {
            return;
        }
        self.input.update(cx, |input, _| input.set_read_only(true));
        let actor = self
            .team_histories
            .as_ref()
            .and_then(|h| h.borrow().get(&self.summary.id).map(|h| h.actor.clone()))
            .or_else(|| self.settings.bot.as_ref().map(|b| b.bot.clone()));
        if let Some(actor) = actor {
            let next = (self.actor_look(&actor), actor.label());
            if self.team_look.as_ref() != Some(&next) {
                self.transcript.update(cx, |t, cx| {
                    t.bot_mode_with_look(next.0.clone(), &next.1);
                    cx.notify();
                });
                self.team_look = Some(next);
                cx.notify();
            }
        }
    }
    pub(crate) fn user_read_only(&self) -> bool {
        self.settings
            .bot
            .as_ref()
            .is_some_and(|b| b.bot.is_minion())
            || self
                .team_histories
                .as_ref()
                .is_some_and(|histories| histories.borrow().contains_key(&self.summary.id))
    }
    pub(super) fn reject_user_entry(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.user_read_only() {
            return false;
        }
        self.status = "Read-only team history. Communicate through the parent Bot.".into();
        cx.notify();
        true
    }
    pub(super) fn actor_look(
        &self,
        bot: &vorkspace_core::bot::Bot,
    ) -> crate::avatar::BotLookConfig {
        if bot.is_minion() {
            let history = self
                .team_histories
                .as_ref()
                .and_then(|h| h.borrow().get(&self.summary.id).cloned());
            return crate::avatar::minion_look(
                history
                    .as_ref()
                    .and_then(|h| h.parent.as_ref())
                    .unwrap_or(bot),
                &bot.id,
            );
        }
        crate::avatar::BotLookConfig::from_bot(bot)
    }
    pub(super) fn render_team_history(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.input.update(cx, |input, _| input.set_read_only(true));
        let history = self
            .team_histories
            .as_ref()
            .and_then(|h| h.borrow().get(&self.summary.id).cloned());
        let actor = history
            .as_ref()
            .map(|h| h.actor.clone())
            .or_else(|| self.settings.bot.as_ref().map(|b| b.bot.clone()));
        let mut body = div()
            .flex_1()
            .min_h_0()
            .child(
                self.transcript
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            )
            .into_any_element();
        if let Some(actor) = actor {
            let look = self.actor_look(&actor);
            body = crate::bot_header::chat_surface_with_look(
                body,
                &actor,
                look,
                self.bot_status(),
                Some(true),
                &mut self.bot_ui.header,
                cx,
            );
        }
        let description = history
            .as_ref()
            .map(|h| {
                if let Some(member) = &h.actor.minion {
                    let parent = h
                        .parent
                        .as_ref()
                        .map(|p| p.label())
                        .unwrap_or_else(|| member.parent_name.clone());
                    format!(
                        "{} · Minion of {}{}{}",
                        member.role,
                        parent,
                        if member.specialties.is_empty() {
                            String::new()
                        } else {
                            format!(" · {}", member.specialties.join(", "))
                        },
                        if member.retired { " · Retired" } else { "" }
                    )
                } else {
                    "Team channel history".into()
                }
            })
            .unwrap_or_else(|| "Minion history".into());
        div()
            .debug_selector(|| "team-read-only-history".into())
            .size_full()
            .flex()
            .flex_col()
            .child(body)
            .child(
                div()
                    .debug_selector(|| "team-history-attribution".into())
                    .px_3()
                    .py_2()
                    .text_size(px(12.))
                    .text_color(content(0.7))
                    .child(description),
            )
            .child(
                div()
                    .px_3()
                    .pb_2()
                    .text_size(px(11.))
                    .text_color(content(0.5))
                    .child("Read-only · Communicate through the parent Bot"),
            )
            // Approval controls retain their exact-request authority; questions go to the parent.
            .child(self.approval_shelf(window, cx))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests;
