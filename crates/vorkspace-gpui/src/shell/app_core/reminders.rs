//! One reminder schedule and ordered write lane per data store.
use super::*;
use crate::notifications::{delivery::Target, platform};
use std::collections::VecDeque;
use std::time::Duration;
use vorkspace_core::store::{Reminder, Store};

#[derive(Clone)]
pub(in crate::shell) enum ReminderEdit {
    Schedule {
        sessions: Vec<i64>,
        due_at: u64,
    },
    Clear {
        session: i64,
        due_at: u64,
    },
    Snooze {
        session: i64,
        due_at: u64,
        until: u64,
    },
}

impl ReminderEdit {
    fn preview(&self, rows: &mut Vec<Reminder>) {
        match self {
            Self::Schedule { sessions, due_at } => {
                rows.retain(|r| !sessions.contains(&r.session_id));
                rows.extend(sessions.iter().map(|&session_id| Reminder {
                    session_id,
                    due_at: *due_at,
                    fired_at: None,
                }));
            }
            Self::Clear { session, due_at } => {
                rows.retain(|r| r.session_id != *session || r.due_at != *due_at);
            }
            Self::Snooze {
                session,
                due_at,
                until,
            } => {
                if let Some(row) = rows
                    .iter_mut()
                    .find(|r| r.session_id == *session && r.due_at == *due_at)
                {
                    row.due_at = *until;
                    row.fired_at = None;
                }
            }
        }
    }

    fn save(&self, store: &mut Store) -> Result<Option<Target>, String> {
        let (session_id, due_at) = match self {
            Self::Schedule { sessions, due_at } => {
                store.set_reminders(sessions, *due_at)?;
                return Ok(None);
            }
            Self::Clear { session, due_at } => {
                store.clear_reminders(&[*session], Some(*due_at))?;
                (*session, *due_at)
            }
            Self::Snooze {
                session,
                due_at,
                until,
            } => {
                if !store.snooze_reminder_if(*session, *due_at, *until)? {
                    return Ok(None);
                }
                (*session, *due_at)
            }
        };
        Ok(Some(Target::Reminder { session_id, due_at }))
    }
}

#[derive(Default)]
pub(super) struct ReminderSchedule {
    saved: Vec<Reminder>,
    optimistic: Option<Vec<Reminder>>,
    queue: VecDeque<ReminderEdit>,
    work: Option<gpui::Task<()>>,
    refresh_after_work: bool,
    deadline: Option<gpui::Task<()>>,
    discovery: Option<gpui::Task<()>>,
    error: Option<String>,
}

impl ReminderSchedule {
    pub(super) fn new(saved: Vec<Reminder>) -> Self {
        Self {
            saved,
            ..Self::default()
        }
    }

    fn rows(&self) -> &[Reminder] {
        self.optimistic.as_deref().unwrap_or(&self.saved)
    }

    fn project(&mut self) {
        self.optimistic = if self.queue.is_empty() {
            None
        } else {
            let mut preview = self.saved.clone();
            for edit in &self.queue {
                edit.preview(&mut preview);
            }
            preview.sort_by_key(|r| (r.due_at, r.session_id));
            Some(preview)
        };
    }

    pub(super) fn stop_discovery(&mut self) {
        self.discovery = None;
        self.deadline = None;
    }
}

struct RemindersChanged {
    fired: Vec<(WeakEntity<Shell>, Reminder, Session)>,
}
impl EventEmitter<RemindersChanged> for AppCore {}

struct Refresh {
    saved: Vec<Reminder>,
    fired: Vec<(Reminder, Session)>,
}

fn refresh(store: &mut Store) -> Result<Refresh, String> {
    let mut saved = store.reminders()?;
    let now = vorkspace_core::time::now_ms();
    let due: HashSet<_> = saved
        .iter()
        .filter(|r| r.fired_at.is_none() && r.due_at <= now)
        .map(|r| r.session_id)
        .collect();
    if due.is_empty() {
        return Ok(Refresh {
            saved,
            fired: Vec::new(),
        });
    }
    let mut summaries: HashMap<_, _> = store
        .sessions()?
        .into_iter()
        .filter(|summary| due.contains(&summary.id))
        .map(|summary| (summary.id, summary))
        .collect();
    let fired = store.claim_due_reminders(&saved, now)?;
    // Re-read after the compare-and-claim. Another Store may have changed
    // the same occurrence while the catalog was being read.
    saved = store.reminders()?;
    let fired = fired
        .into_iter()
        .filter(|r| {
            saved.iter().any(|current| {
                current.session_id == r.session_id
                    && current.due_at == r.due_at
                    && current.fired_at == Some(now)
            })
        })
        .filter_map(|r| {
            let summary = summaries.remove(&r.session_id)?;
            Some((r, summary))
        })
        .collect();
    Ok(Refresh { saved, fired })
}

impl AppCore {
    pub(super) fn start_reminders(&mut self, cx: &mut Context<Self>) {
        if self.reminders.discovery.is_some() {
            return;
        }
        self.refresh_reminders(cx);
        self.reminders.discovery = Some(cx.spawn(async move |core, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(30))
                    .await;
                if core
                    .update(cx, |core, cx| core.refresh_reminders(cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    pub(in crate::shell) fn edit_reminder(&mut self, edit: ReminderEdit, cx: &mut Context<Self>) {
        self.reminders.queue.push_back(edit);
        self.reminders.project();
        cx.emit(RemindersChanged { fired: Vec::new() });
        self.refresh_reminders(cx);
    }

    pub(in crate::shell) fn retry_reminders(&mut self, cx: &mut Context<Self>) {
        if self.reminders.error.take().is_some() {
            cx.emit(RemindersChanged { fired: Vec::new() });
        }
        self.refresh_reminders(cx);
    }

    pub(in crate::shell) fn refresh_reminders(&mut self, cx: &mut Context<Self>) {
        if self.reminders.work.is_some() {
            self.reminders.refresh_after_work = true;
            return;
        }
        if self.reminders.queue.is_empty() && self.reminders.error.is_some() {
            return;
        }
        self.reminders.deadline = None;
        let store = self.store.clone();
        let edit = self.reminders.queue.front().cloned();
        let editing = edit.is_some();
        let work = cx.background_executor().spawn(async move {
            let mut store = locked(&store)?;
            let dismiss = edit
                .map(|edit| edit.save(&mut store))
                .transpose()
                .map(Option::flatten);
            Ok::<_, String>((dismiss, refresh(&mut store)))
        });
        self.reminders.work = Some(cx.spawn(async move |core, cx| {
            let result = work.await;
            let _ = core.update(cx, |core, cx| {
                core.reminders.work = None;
                let prior = core.reminders.rows().to_vec();
                let prior_error = core.reminders.error.take();
                if editing {
                    core.reminders.queue.pop_front();
                }
                let mut fired = Vec::new();
                match result {
                    Ok((dismiss, result)) => {
                        match dismiss {
                            Ok(Some(target)) => platform::dismiss(&target.tag(0)),
                            Ok(None) => {}
                            Err(error) => core.reminders.error = Some(error),
                        }
                        match result {
                            Ok(refresh) => {
                                core.reminders.saved = refresh.saved;
                                for (reminder, summary) in refresh.fired {
                                    // A newer intent may already be queued. Let that edit
                                    // win before issuing an obsolete OS notification.
                                    if core
                                        .reminders
                                        .queue
                                        .iter()
                                        .any(|edit| edit.supersedes(&reminder))
                                    {
                                        continue;
                                    }
                                    if let Some(owner) = core.route(reminder.session_id, cx) {
                                        fired.push((owner.downgrade(), reminder, summary));
                                    }
                                }
                            }
                            Err(error) => {
                                core.reminders.error =
                                    Some(format!("Could not deliver reminders: {error}"))
                            }
                        }
                    }
                    Err(error) => core.reminders.error = Some(error),
                }
                core.reminders.project();
                if core.reminders.rows() != prior
                    || core.reminders.error != prior_error
                    || !fired.is_empty()
                {
                    cx.emit(RemindersChanged { fired });
                }
                let refresh_again = std::mem::take(&mut core.reminders.refresh_after_work);
                if !core.reminders.queue.is_empty() || refresh_again {
                    core.refresh_reminders(cx);
                } else {
                    core.schedule_reminder_deadline(cx);
                }
            });
        }));
    }

    fn schedule_reminder_deadline(&mut self, cx: &mut Context<Self>) {
        if self.reminders.discovery.is_none() || self.reminders.error.is_some() {
            return;
        }
        let next = self
            .reminders
            .saved
            .iter()
            .filter(|r| r.fired_at.is_none())
            .map(|r| r.due_at)
            .min();
        self.reminders.deadline = next.map(|at| {
            cx.spawn(async move |core, cx| {
                let delay = at.saturating_sub(vorkspace_core::time::now_ms());
                cx.background_executor()
                    .timer(Duration::from_millis(delay))
                    .await;
                let _ = core.update(cx, |core, cx| core.refresh_reminders(cx));
            })
        });
    }
}

impl ReminderEdit {
    fn supersedes(&self, reminder: &Reminder) -> bool {
        match self {
            Self::Schedule { sessions, .. } => sessions.contains(&reminder.session_id),
            Self::Clear { session, due_at }
            | Self::Snooze {
                session, due_at, ..
            } => *session == reminder.session_id && *due_at == reminder.due_at,
        }
    }
}

impl Shell {
    pub(in crate::shell) fn reminders<'a>(&self, cx: &'a App) -> &'a [Reminder] {
        self.app_core
            .as_ref()
            .map_or(&[], |core| core.read(cx).reminders.rows())
    }

    pub(in crate::shell) fn reminder_error<'a>(&self, cx: &'a App) -> Option<&'a str> {
        self.app_core.as_ref()?.read(cx).reminders.error.as_deref()
    }

    pub(in crate::shell) fn edit_reminder(&self, edit: ReminderEdit, cx: &mut Context<Self>) {
        if let Some(core) = &self.app_core {
            core.update(cx, |core, cx| core.edit_reminder(edit, cx));
        }
    }

    pub(in crate::shell) fn poll_reminders(&self, cx: &mut Context<Self>) {
        if let Some(core) = &self.app_core {
            core.update(cx, |core, cx| core.refresh_reminders(cx));
        }
    }

    pub(super) fn observe_reminders(&mut self, core: &Entity<AppCore>, cx: &mut Context<Self>) {
        self.observations.push(
            cx.subscribe(core, |shell, _, event: &RemindersChanged, cx| {
                for (owner, reminder, summary) in &event.fired {
                    if *owner == cx.weak_entity() {
                        shell
                            .session_summaries
                            .entry(summary.id)
                            .or_insert_with(|| summary.clone());
                        shell.deliver_reminder_notice(summary, reminder.due_at, cx);
                    }
                }
                shell.invalidate_chrome(cx);
                cx.notify();
            }),
        );
    }
}
