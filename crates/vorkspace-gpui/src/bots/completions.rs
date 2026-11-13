//! Delegated results belong to one source turn and exact child notices.
use super::*;
use bot::{Launch, LaunchGroup};

struct Child {
    session: i64,
    title: String,
    notice: Option<String>,
}

fn pending(launch: &Launch) -> bool {
    launch.notify_on_complete && !launch.completed && !launch.dismissed
}

fn completion_positions(bot: &Bot, group: LaunchGroup, children: &[Child]) -> Option<Vec<usize>> {
    let pending_children = bot
        .launches
        .iter()
        .enumerate()
        .filter(|(_, launch)| pending(launch) && bot.launch_group(launch) == group)
        .collect::<Vec<_>>();
    pending_children
        .iter()
        .map(|(_, launch)| (launch.session_id, launch.notification_id.as_deref()))
        .eq(children
            .iter()
            .map(|child| (child.session, child.notice.as_deref())))
        .then(|| {
            pending_children
                .into_iter()
                .map(|(position, _)| position)
                .collect()
        })
}

impl Bots {
    fn source_busy(&self, group: LaunchGroup, cx: &App) -> bool {
        let Some(source) = group.source() else {
            return false;
        };
        self.chats
            .values()
            .chain(self.running.get(&source).map(|run| &run.view))
            .any(|view| {
                let view = view.read(cx);
                view.summary.id == source && view.turn_in_flight()
            })
    }

    pub(super) fn check_completions(&mut self, cx: &mut Context<Self>) {
        if !self
            .roster
            .bots
            .iter()
            .any(|bot| bot.launches.iter().any(pending))
        {
            return;
        }
        let waiting = self
            .owner
            .session_views(cx)
            .into_iter()
            .filter(|session| session.busy || !session.has_messages)
            .map(|session| session.id)
            .collect::<HashSet<_>>();
        let mut ready = Vec::new();
        for bot in &self.roster.bots {
            let mut groups = HashMap::<LaunchGroup, Vec<&Launch>>::new();
            for launch in bot.launches.iter().filter(|launch| pending(launch)) {
                groups
                    .entry(bot.launch_group(launch))
                    .or_default()
                    .push(launch);
            }
            for (group, launches) in groups {
                if launches
                    .iter()
                    .any(|launch| waiting.contains(&launch.session_id))
                    || self.source_busy(group, cx)
                    || self.completion_work.contains(&(bot.id.clone(), group))
                {
                    continue;
                }
                let children = launches
                    .into_iter()
                    .map(|launch| Child {
                        session: launch.session_id,
                        title: launch.title.clone(),
                        notice: launch.notification_id.clone(),
                    })
                    .collect::<Vec<_>>();
                ready.push((bot.id.clone(), group, children));
            }
        }
        for (id, group, children) in ready {
            self.completion_work.insert((id.clone(), group));
            let store = self.store.clone();
            let work = cx.background_executor().spawn(async move {
                let report = children
                    .iter()
                    .map(|child| {
                        let rows = locked(&store)
                            .and_then(|db| db.load(child.session))
                            .map(|t| vorkspace_core::response::prose(&t.rows))
                            .unwrap_or_else(|_| "Session was removed".into());
                        format!(
                            "Session {}: {}\n{}",
                            child.session,
                            child.title,
                            vorkspace_core::response::limit(&rows, 12000)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                (children, report)
            });
            cx.spawn(async move |this, cx| {
                let (children, report) = work.await;
                let _ = this.update(cx, |this, cx| {
                    this.completion_work.remove(&(id.clone(), group));
                    this.publish_completion(id, group, children, report, cx);
                });
            })
            .detach();
        }
    }

    fn publish_completion(
        &mut self,
        id: String,
        group: LaunchGroup,
        children: Vec<Child>,
        report: String,
        cx: &mut Context<Self>,
    ) {
        if !self.roster.is_active(&id) || self.source_busy(group, cx) {
            return;
        }
        let Some(bot) = self.roster.get(&id) else {
            return;
        };
        let Some(positions) = completion_positions(bot, group, &children) else {
            return;
        };
        if self.owner.session_views(cx).into_iter().any(|view| {
            (view.busy || !view.has_messages)
                && children.iter().any(|child| child.session == view.id)
        }) {
            return;
        }
        let bot = self.roster.get_mut(&id).expect("validated Bot identity");
        for position in positions {
            bot.launches[position].completed = true;
        }
        let message = format!(
            "[Delegated work completed]\nAll sessions in this group have stopped. Inspect their results and give an integrated update.\n\n{report}"
        );
        if let Some(source) = group.source()
            && let Some(run) = self.running.get_mut(&source).filter(|run| run.bot == id)
        {
            run.mailbox.push(message);
            self.save(cx);
            self.habit_changed(source, cx);
        } else {
            self.enqueue(&id, message, Vec::new(), true, cx);
            self.ensure_chat(&id, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delayed_report_cannot_complete_a_replacement_notice_or_a_new_sibling() {
        let source = (1_i64 << 52) | 13;
        let mut launch = Launch {
            session_id: 5,
            notify_on_complete: true,
            ..Default::default()
        };
        launch.set_origin(source, 99);
        let mut bot = Bot {
            launches: vec![launch],
            ..Default::default()
        };
        let group = bot.launch_group(&bot.launches[0]);
        let children = [Child {
            session: 5,
            title: "Child".into(),
            notice: bot.launches[0].notification_id.clone(),
        }];
        assert_eq!(completion_positions(&bot, group, &children), Some(vec![0]));
        let mut sibling = bot.launches[0].clone();
        sibling.session_id = 7;
        sibling.set_origin(source, 99);
        bot.launches.push(sibling);
        assert_eq!(completion_positions(&bot, group, &children), None);
        bot.launches.pop();
        bot.launches[0].set_origin(source, 99);
        assert_eq!(completion_positions(&bot, group, &children), None);
    }
}
