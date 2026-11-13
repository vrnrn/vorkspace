//! Read selected Bot work metadata in the background, once per source revision.
use super::*;
type Revision = (u64, u64, u64);
#[derive(Default)]
pub(super) struct Cache {
    revisions: HashMap<String, Revision>,
    pending: HashSet<String>,
    checked: HashMap<String, std::time::Instant>,
}
impl Bots {
    pub(super) fn refresh_work(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.work.pending.contains(id)
            || self
                .work
                .checked
                .get(id)
                .is_some_and(|at| at.elapsed() < Duration::from_secs(1))
        {
            return;
        }
        let Some(session) = self.roster.get(id).and_then(|m| m.session_id) else {
            return;
        };
        self.work.pending.insert(id.into());
        self.work
            .checked
            .insert(id.into(), std::time::Instant::now());
        let previous = self.work.revisions.get(id).copied();
        let store = self.store.clone();
        let id = id.to_owned();
        let work = cx.background_executor().spawn(async move {
            let db = locked(&store)?;
            let revision = db.bot_work_revision(session)?;
            let data = if revision.is_some() && revision != previous {
                db.bot_work(session)?
            } else {
                None
            };
            Ok::<_, String>((revision, data))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.work.pending.remove(&id);
                let (revision, data) = match result {
                    Ok(value) => value,
                    Err(error) => {
                        this.error = Some(format!("Could not refresh this Bot’s work: {error}"));
                        cx.notify();
                        return;
                    }
                };
                if let Some(revision) = revision {
                    this.work.revisions.insert(id.clone(), revision);
                }
                let Some((documents, launches)) = data else {
                    return;
                };
                let Some(bot) = this
                    .roster
                    .get_mut(&id)
                    .filter(|m| m.session_id == Some(session))
                else {
                    return;
                };
                let changed = bot.apply_work(documents, launches);
                if changed {
                    this.configure_chat(&id, cx);
                    this.refresh_cards(cx);
                    cx.emit(Changed);
                    cx.notify();
                }
            });
        })
        .detach();
    }
}
