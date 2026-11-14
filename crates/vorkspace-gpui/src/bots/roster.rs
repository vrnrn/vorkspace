//! Committed Bot state and ordered writes for every window on a store.
use super::{
    Bots,
    effects::{MinionEffects, Permit},
};
use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Task, WeakEntity};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};
use vorkspace_core::{
    agent::{AccessMode, Settings},
    bot::{Roster, home},
    provider::Kind,
    store::{Session, Store},
};
use vorkspace_runtime::runtime::{Database, locked};
use vorkspace_ui::preferences::Preferences;

pub(super) type PreparedChat = (Session, Option<Settings>, Vec<String>, Option<String>);

pub(crate) struct MembershipChanged {
    pub(crate) ids: Vec<String>,
    pub(crate) vorkspace: Option<String>,
}

pub(crate) struct SharedRoster {
    pub(super) store: Database,
    pub(super) root: PathBuf,
    pub(super) snapshot: Result<Roster, String>,
    pub(super) revision: u64,
    // A pending local creation and a committed deletion can both be absent
    // from the snapshot. Keep the deletion revision to distinguish them.
    pub(super) removed: HashMap<String, u64>,
    chat_owners: HashMap<String, WeakEntity<Bots>>,
    habit_runner: Option<WeakEntity<Bots>>,
    // New ordinary messages can hand off within this process. Leave older
    // queues on the existing cold recovery policy after app restart.
    live_messages: HashSet<String>,
    minion_effects: MinionEffects,
    minions_enabled: bool,
    writes: Option<Task<()>>,
}

impl EventEmitter<MembershipChanged> for SharedRoster {}

pub(crate) struct PreparedRoster {
    snapshot: Result<Roster, String>,
    root: PathBuf,
}

impl PreparedRoster {
    pub(crate) fn read(store: &Store) -> Self {
        Self {
            snapshot: Roster::load(store),
            root: vorkspace_core::store::bot_root(store),
        }
    }
}

impl SharedRoster {
    pub(crate) fn internal_session_ids(&self) -> impl Iterator<Item = i64> + '_ {
        self.snapshot.iter().flat_map(|roster| {
            roster
                .hidden_sessions
                .iter()
                .copied()
                .chain(roster.bots.iter().flat_map(|bot| {
                    bot.session_id
                        .into_iter()
                        .chain(bot.habit_sessions.iter().copied())
                }))
        })
    }

    pub(crate) fn install(store: Database, cx: &mut App) -> Entity<Self> {
        let prepared = match locked(&store) {
            Ok(db) => PreparedRoster::read(&db),
            Err(error) => PreparedRoster {
                snapshot: Err(error),
                root: std::env::temp_dir().join(vorkspace_core::bot::new_id()),
            },
        };
        Self::install_prepared(store, prepared, cx)
    }

    pub(crate) fn install_prepared(
        store: Database,
        prepared: PreparedRoster,
        cx: &mut App,
    ) -> Entity<Self> {
        let shared = cx.new(|_| Self {
            store,
            root: prepared.root,
            snapshot: prepared.snapshot,
            revision: 0,
            removed: HashMap::new(),
            chat_owners: HashMap::new(),
            habit_runner: None,
            live_messages: HashSet::new(),
            minion_effects: MinionEffects::default(),
            minions_enabled: false,
            writes: None,
        });
        let quit = shared.downgrade();
        cx.on_app_quit(move |cx| {
            let writes = quit
                .update(cx, |shared, _| shared.writes.take())
                .ok()
                .flatten();
            async move {
                if let Some(writes) = writes {
                    writes.await;
                }
            }
        })
        .detach();
        shared
    }

    pub(crate) fn member_ids(&self) -> Option<Vec<String>> {
        self.snapshot.as_ref().ok().map(Self::ids)
    }

    pub(crate) fn committed(&self) -> Option<&Roster> {
        self.snapshot.as_ref().ok()
    }

    pub(crate) fn has_pending_writes(&self) -> bool {
        self.writes.as_ref().is_some_and(|task| !task.is_ready())
    }

    pub(crate) fn write_barrier(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<async_channel::Receiver<()>> {
        if !self.has_pending_writes() {
            return None;
        }
        let previous = self.writes.take().expect("pending roster writer");
        let (send, receive) = async_channel::bounded(1);
        self.writes = Some(cx.spawn(async move |_, _| {
            previous.await;
            let _ = send.try_send(());
        }));
        Some(receive)
    }

    pub(super) fn chat_owner(&self, id: &str) -> Option<&WeakEntity<Bots>> {
        self.chat_owners
            .get(id)
            .filter(|owner| owner.upgrade().is_some())
    }

    pub(super) fn claim_chat(&mut self, id: &str, owner: WeakEntity<Bots>) -> bool {
        if let Some(holder) = self.chat_owner(id) {
            return holder == &owner;
        }
        self.chat_owners
            .retain(|_, owner| owner.upgrade().is_some());
        self.chat_owners.insert(id.into(), owner);
        true
    }

    pub(super) fn release_chat(&mut self, id: &str, owner: &WeakEntity<Bots>) {
        if self.chat_owners.get(id) == Some(owner) {
            self.chat_owners.remove(id);
        }
    }

    pub(super) fn claim_habits(&mut self, owner: WeakEntity<Bots>) -> bool {
        if let Some(holder) = &self.habit_runner
            && holder.upgrade().is_some()
        {
            return holder == &owner;
        }
        self.habit_runner = Some(owner);
        true
    }

    pub(super) fn live_message(&self, id: &str) -> bool {
        self.live_messages.contains(id)
    }

    pub(super) fn configure_minions(&mut self, enabled: bool) {
        if self.minions_enabled != enabled {
            self.minions_enabled = enabled;
            if let Ok(roster) = &self.snapshot {
                self.minion_effects.reconcile(roster, enabled);
            }
        }
    }

    pub(super) fn minion_permit(&self, actor: &str, projects: &[String]) -> Result<Permit, String> {
        self.minion_effects.permit(actor, projects)
    }

    pub(super) fn minion_fences(&self, actor: &str) -> Vec<Permit> {
        self.minion_effects.fences(actor)
    }

    pub(super) fn retire_minion(
        &mut self,
        parent: String,
        id: String,
        cx: &mut Context<Self>,
    ) -> async_channel::Receiver<Result<Option<i64>, String>> {
        self.write(None, cx, move |db| {
            let mut roster = Roster::load(db)?;
            roster.retire_minion(&parent, &id)?;
            let session = roster.get(&id).and_then(|bot| bot.session_id);
            Ok((Some(roster.save_snapshot(db)?), Ok(session)))
        })
    }

    pub(super) fn delete_bot(
        &mut self,
        id: String,
        cx: &mut Context<Self>,
    ) -> async_channel::Receiver<Result<(), String>> {
        self.write(None, cx, move |db| {
            let mut roster = Roster::load(db)?;
            if roster.get(&id).is_none() {
                return Ok((None, Ok(())));
            }
            roster.bots.retain(|bot| bot.id != id);
            roster.sanitize_team_queues();
            let live: HashSet<_> = roster
                .bots
                .iter()
                .flat_map(|bot| bot.outbox.iter().map(|message| &message.id))
                .collect();
            roster.held_messages.retain(|id| live.contains(id));
            roster.message_modes.retain(|id, _| live.contains(id));
            Ok((Some(roster.save_snapshot(db)?), Ok(())))
        })
    }

    /// A Bot reaches every project of its vorkspace. The vorkspace definitions
    /// own that list; `Bot.projects` is the copy the core and providers read.
    /// Bots absent from `projects` have not been placed yet and are left alone.
    pub(crate) fn follow_vorkspace_projects(
        &mut self,
        projects: HashMap<String, Vec<String>>,
        cx: &mut Context<Self>,
    ) {
        fn stale(roster: &Roster, projects: &HashMap<String, Vec<String>>) -> bool {
            roster.bots.iter().any(|bot| {
                !bot.is_minion() && projects.get(&bot.id).is_some_and(|p| *p != bot.projects)
            })
        }
        if !self
            .committed()
            .is_some_and(|roster| stale(roster, &projects))
        {
            return;
        }
        let _ = self.write(None, cx, move |db| {
            let mut roster = Roster::load(db)?;
            if !stale(&roster, &projects) {
                return Ok((None, Ok(())));
            }
            for bot in roster.bots.iter_mut().filter(|bot| !bot.is_minion()) {
                if let Some(projects) = projects.get(&bot.id) {
                    bot.projects.clone_from(projects);
                }
            }
            Ok((Some(roster.save_snapshot(db)?), Ok(())))
        });
    }

    fn ids(roster: &Roster) -> Vec<String> {
        roster
            .bots
            .iter()
            .filter(|bot| !bot.is_minion())
            .map(|bot| bot.id.clone())
            .collect()
    }

    /// Preserve command receipt ordering while publishing the roster even
    /// when the originating window closes or its later receipt write fails.
    pub(super) fn save<T: Send + 'static>(
        &mut self,
        roster: Roster,
        vorkspace: Option<String>,
        finalize: impl FnOnce(&Store) -> Result<T, String> + Send + 'static,
        cx: &mut Context<Self>,
    ) -> async_channel::Receiver<Result<T, String>> {
        self.write(vorkspace, cx, move |db| {
            let snapshot = roster.save_snapshot(db)?;
            Ok((Some(snapshot), finalize(db)))
        })
    }

    /// Resolve a backing identity under the same writer that publishes the
    /// roster. A window may disappear while this job is queued or in flight.
    pub(super) fn prepare_chat(
        &mut self,
        id: String,
        prefs: Preferences,
        cx: &mut Context<Self>,
    ) -> async_channel::Receiver<Result<PreparedChat, String>> {
        let root = self.root.clone();
        self.write(None, cx, move |db| {
            let mut roster = Roster::load(db)?;
            let bot = roster.get(&id).ok_or("Bot is no longer available")?;
            if let Some(missing) = &bot.unavailable_session {
                return Err(format!(
                    "This Bot's saved conversation ({missing}) is unavailable."
                ));
            }
            if let Some(session) = bot.session_id {
                let chat = super::load_backing_chat(db, &root, bot, session)?;
                return Ok((None, Ok(chat)));
            }
            let bot = bot.clone();
            let home = home::ensure(&root, &bot, None)?;
            let provider = match &bot.initial_configuration {
                Some(config) => Kind::parse(&config.provider)
                    .ok_or("This Bot's configured provider is unavailable.")?,
                None => prefs
                    .default_provider
                    .as_deref()
                    .and_then(Kind::parse)
                    .unwrap_or(Kind::Codex),
            };
            let settings = bot
                .initial_configuration
                .as_ref()
                .map(|config| config.settings.clone())
                .unwrap_or_else(|| Settings {
                    access: AccessMode::Auto,
                    model: prefs.default_models.get(provider.id()).cloned(),
                    ..Default::default()
                });
            let mut summary = db.create(provider.id(), &home.to_string_lossy())?;
            summary.title = bot.label();
            let committed = (|| {
                db.title(summary.id, &summary.title)?;
                db.configure_agent(summary.id, provider.id(), &settings)?;
                let entry = roster.get_mut(&id).expect("loaded Bot");
                entry.session_id = Some(summary.id);
                entry.initial_configuration = None;
                roster.hidden_sessions.push(summary.id);
                roster.save_snapshot(db)
            })();
            let snapshot = match committed {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    if let Err(cleanup) = db.delete(summary.id) {
                        return Err(format!(
                            "{error}. Could not clean up the backing conversation: {cleanup}"
                        ));
                    }
                    return Err(error);
                }
            };
            let warning = home::write_session(&root, &bot, &summary, &settings.account).err();
            Ok((
                Some(snapshot),
                Ok((summary, Some(settings), Vec::new(), warning)),
            ))
        })
    }

    fn write<T: Send + 'static>(
        &mut self,
        vorkspace: Option<String>,
        cx: &mut Context<Self>,
        work: impl FnOnce(&mut Store) -> Result<(Option<Roster>, Result<T, String>), String>
        + Send
        + 'static,
    ) -> async_channel::Receiver<Result<T, String>> {
        let store = self.store.clone();
        let previous = self.writes.take();
        let (send, receive) = async_channel::bounded(1);
        self.writes = Some(cx.spawn(async move |shared, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            let work = cx
                .background_executor()
                .spawn(async move { work(&mut *locked(&store)?) });
            let result = match work.await {
                Ok((Some(snapshot), result)) => {
                    let _ = shared.update(cx, |shared, cx| {
                        let ids = Self::ids(&snapshot);
                        let changed = shared.member_ids().as_ref() != Some(&ids);
                        shared.revision += 1;
                        let mut retired = Vec::new();
                        let messages = |roster: &Roster| {
                            roster
                                .bots
                                .iter()
                                .flat_map(|bot| {
                                    bot.outbox
                                        .iter()
                                        .filter(|message| message.peer.is_none())
                                        .map(|message| message.id.clone())
                                })
                                .collect::<HashSet<_>>()
                        };
                        let current = messages(&snapshot);
                        shared.live_messages.retain(|id| current.contains(id));
                        if let Ok(previous) = &shared.snapshot {
                            shared
                                .live_messages
                                .extend(current.difference(&messages(previous)).cloned());
                            for bot in &previous.bots {
                                if snapshot.get(&bot.id).is_none() {
                                    shared.removed.insert(bot.id.clone(), shared.revision);
                                }
                                if bot.is_minion()
                                    && previous.is_active(&bot.id)
                                    && !snapshot.is_active(&bot.id)
                                    && let Some(owner) = shared.chat_owner(&bot.id)
                                {
                                    retired.push((bot.id.clone(), owner.clone()));
                                }
                            }
                            shared.removed.retain(|id, _| snapshot.get(id).is_none());
                            shared.chat_owners.retain(|id, _| {
                                previous.get(id).is_none() || snapshot.get(id).is_some()
                            });
                        }
                        shared
                            .minion_effects
                            .reconcile(&snapshot, shared.minions_enabled);
                        shared.snapshot = Ok(snapshot);
                        for (id, owner) in retired {
                            let _ = owner.update(cx, |bots, cx| bots.stop_retired_chat(&id, cx));
                        }
                        if changed {
                            cx.emit(MembershipChanged { ids, vorkspace });
                        }
                        cx.notify();
                    });
                    result
                }
                Ok((None, result)) => result,
                Err(error) => Err(error),
            };
            let _ = send.try_send(result);
        }));
        receive
    }
}
