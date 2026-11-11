use super::*;
use crate::conversation::BotSubmitted;

struct AttachedChat {
    view: Entity<Conversation>,
    _subscriptions: Vec<Subscription>,
}

/// A chat's observer lifetime follows its attachment, including when another
/// pane still holds the old view after reset, replacement, or retirement.
#[derive(Default)]
pub(super) struct Chats(HashMap<String, AttachedChat>);

impl Chats {
    pub(super) fn get(&self, id: &str) -> Option<&Entity<Conversation>> {
        self.0.get(id).map(|chat| &chat.view)
    }

    pub(super) fn contains_key(&self, id: &str) -> bool {
        self.0.contains_key(id)
    }

    pub(super) fn keys(&self) -> impl Iterator<Item = &String> {
        self.0.keys()
    }

    pub(super) fn values(&self) -> impl Iterator<Item = &Entity<Conversation>> {
        self.0.values().map(|chat| &chat.view)
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (&String, &Entity<Conversation>)> {
        self.0.iter().map(|(id, chat)| (id, &chat.view))
    }

    pub(super) fn remove(&mut self, id: &str) -> Option<Entity<Conversation>> {
        self.0.remove(id).map(|chat| chat.view)
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.0.len()
    }
}

impl std::ops::Index<&str> for Chats {
    type Output = Entity<Conversation>;

    fn index(&self, id: &str) -> &Self::Output {
        &self.0[id].view
    }
}

impl Bots {
    pub(super) fn attach_chat(
        &mut self,
        id: String,
        view: Entity<Conversation>,
        cx: &mut Context<Self>,
    ) {
        self.apply_chat_visibility(&id, &view, cx);
        let subscriptions = Self::observe_chat(&id, &view, cx);
        self.chats.0.insert(
            id,
            AttachedChat {
                view,
                _subscriptions: subscriptions,
            },
        );
    }

    fn observe_chat(
        id: &str,
        view: &Entity<Conversation>,
        cx: &mut Context<Self>,
    ) -> Vec<Subscription> {
        let mut subscriptions = Vec::new();
        let source = id.to_owned();
        subscriptions.push(
            cx.subscribe(view, move |this, _, event: &BotSubmitted, cx| {
                this.enqueue_held(
                    &source,
                    event.text.clone(),
                    event
                        .attachments
                        .iter()
                        .map(|p| p.to_string_lossy().to_string())
                        .collect(),
                    false,
                    event.queue,
                    event.draft_id.clone(),
                    event.mode.clone(),
                    event.prepared_prompt.clone(),
                    cx,
                )
            }),
        );
        let source = id.to_owned();
        subscriptions.push(cx.subscribe(view, move |this, _, _: &SessionChanged, cx| {
            this.chat_changed(&source, cx);
        }));
        let source = id.to_owned();
        subscriptions.push(cx.subscribe(
            view,
            move |this, _, _: &crate::conversation::UsageResumed, cx| {
                let empty = this.roster.get(&source).is_none_or(|m| m.outbox.is_empty());
                if empty {
                    this.enqueue(
                        &source,
                        "Continue from where you left off.".into(),
                        Vec::new(),
                        false,
                        cx,
                    );
                } else {
                    if let Some(bot) = this.roster.get_mut(&source) {
                        for message in &mut bot.outbox {
                            // Usage recovery may be automatic. Only an
                            // explicit Retry can authorize a new peer
                            // submission after uncertain external effects.
                            if message.peer.is_none() {
                                message.error = None;
                            }
                        }
                    }
                    this.save(cx);
                    this.configure_chat(&source, cx);
                }
            },
        ));
        let source = id.to_owned();
        let mut status = view.read(cx).bot_status_label();
        subscriptions.push(cx.observe(view, move |this, view, cx| {
            let next = view.read(cx).bot_status_label();
            if this.active.as_ref() == Some(&source) || status != next {
                cx.notify();
            }
            status = next;
            this.dispatch(&source, cx);
        }));
        let source = id.to_owned();
        subscriptions.push(cx.subscribe(
            view,
            move |this, _, event: &crate::conversation::BotSteerFailed, cx| {
                this.steer_failed(&source, &event.id, &event.error, cx)
            },
        ));
        let source = id.to_owned();
        subscriptions.push(cx.subscribe(
            view,
            move |this, _, event: &crate::conversation::BotQueueChanged, cx| {
                this.queue_changed(&source, &event.id, event.remove, cx)
            },
        ));
        let transcript = view.read(cx).transcript.clone();
        let target = view.entity_id();
        let source = id.to_owned();
        subscriptions.push(cx.subscribe(
            &transcript,
            move |this, _, event: &crate::transcript::BotResponseAction, cx| match event {
                crate::transcript::BotResponseAction::Queue { id, remove } => {
                    this.queue_changed(&source, id, *remove, cx)
                }
                crate::transcript::BotResponseAction::Quote { text, window } => {
                    let owner = cx.weak_entity();
                    let source = source.clone();
                    let text = text.clone();
                    let handle = *window;
                    cx.defer(move |cx| {
                        let Some(view) = owner
                            .upgrade()
                            .and_then(|owner| owner.read(cx).chats.get(&source).cloned())
                            .filter(|view| view.entity_id() == target)
                        else {
                            return;
                        };
                        let _ = handle.update(cx, |_, window, cx| {
                            view.update(cx, |v, cx| v.bot_quote(&text, window, cx))
                        });
                    });
                }
            },
        ));
        let source = id.to_owned();
        subscriptions.push(cx.subscribe(
            &transcript,
            move |this, _, event: &crate::transcript::bot_cards::CardAction, cx| {
                this.card_action(&source, &event.0, cx)
            },
        ));
        subscriptions
    }
}
