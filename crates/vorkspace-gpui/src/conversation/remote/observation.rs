//! Transfer ownership of the wire snapshot to one in-flight idle refresh.
use super::*;
use crate::transcript::PreparedHistory;
use std::collections::HashMap;
use vorkspace_core::{
    agent::TurnInfo,
    remote::Connections,
    store::{RemoteSession, Store},
};

pub(super) struct Refresh {
    pub(super) observation: SessionObservation,
    pub(super) binding: Option<RemoteSession>,
    pub(super) result: Result<Snapshot, String>,
}

pub(super) struct Snapshot {
    pub(super) settings: Settings,
    pub(super) descriptor: Option<Value>,
    pub(super) history: Option<Projection>,
    pub(super) metadata_changed: bool,
}

pub(super) struct Projection {
    pub(super) history: PreparedHistory,
    pub(super) turns: HashMap<usize, TurnInfo>,
}

fn binding(store: &Store, id: i64) -> Result<RemoteSession, String> {
    store
        .remote_session(id)?
        .ok_or_else(|| "Remote session no longer exists".into())
}

fn block_count(observation: &SessionObservation) -> usize {
    observation
        .snapshot()
        .and_then(|value| value["session"]["blocks"].as_array())
        .map_or(0, Vec::len)
}

impl Refresh {
    fn read(
        database: Database,
        id: i64,
        client: Option<Arc<Connections>>,
        mut observation: SessionObservation,
        initial: bool,
    ) -> Self {
        let mut local = None;
        let mut advanced = false;
        let result = (|| {
            local = Some(binding(&*locked(&database)?, id)?);
            let before = local.as_ref().unwrap();
            let client = client.ok_or("Remote connections are unavailable")?;
            let machine = before.machine(&client)?;
            let descriptor = initial
                .then(|| client.request(&machine.id, "environment.describe", json!({})))
                .transpose()?;
            if observation.snapshot().is_some_and(|snapshot| {
                snapshot["session"]["id"].as_str() != before.session_id.as_deref()
            }) {
                observation = SessionObservation::default();
            }
            let previous_count = block_count(&observation);
            let first_changed = before
                .session_id
                .as_deref()
                .map(|session| client.poll_session(&machine.id, session, &mut observation))
                .transpose()?
                .flatten();
            advanced = first_changed.is_some();
            let (latest, settings) = {
                let store = locked(&database)?;
                let mut latest = binding(&store, id)?;
                // A deleted or rebound desktop handle cannot publish an
                // observation admitted for its previous Host session.
                if latest.session_id != before.session_id
                    || latest.project.environment_id != before.project.environment_id
                    || latest.project.project_id != before.project.project_id
                {
                    local = None;
                    return Err("Remote session changed while loading".into());
                }
                if advanced && let Some(snapshot) = observation.snapshot() {
                    let summary = store.register_remote_summary(&latest.project, snapshot)?;
                    latest = binding(&store, summary.id)?;
                }
                let settings = store
                    .agent_preferences(latest.summary.id)?
                    .unwrap_or_default();
                (latest, settings)
            };
            local = Some(latest);
            let remote = local.as_ref().unwrap();
            let history_changed = first_changed
                .is_some_and(|first| first < previous_count.max(block_count(&observation)));
            let history = if initial || history_changed {
                let (history, turns) = observation
                    .snapshot()
                    .zip(remote.session_id.as_deref())
                    .map(|(snapshot, id)| {
                        Store::project_remote_history(snapshot, &remote.project.environment_id, id)
                    })
                    .transpose()?
                    .unwrap_or_default();
                Some(Projection {
                    history: PreparedHistory::new(history.rows, false),
                    turns,
                })
            } else {
                None
            };
            Ok(Snapshot {
                settings,
                descriptor,
                history,
                metadata_changed: initial || advanced,
            })
        })();
        if advanced && result.is_err() {
            // A later unchanged reply cannot confirm a projection that this
            // refresh could not finish.
            observation = SessionObservation::default();
        }
        Self {
            observation,
            binding: local,
            result,
        }
    }
}

impl Conversation {
    pub(in crate::conversation) fn refresh_remote_snapshot(&mut self, cx: &mut Context<Self>) {
        if self.remote_state.refreshing || self.busy || self.stopping {
            return;
        }
        self.remote_state.refreshing = true;
        self.remote_state.refresh_generation = self.remote_state.refresh_generation.wrapping_add(1);
        let refresh = self.remote_state.refresh_generation;
        let submission = self.submission_generation;
        let initial = !self.loaded || self.history_unloaded;
        self.loading = initial;
        let store = self.store.clone();
        let id = self.summary.id;
        let observation = std::mem::take(&mut self.remote_state.observation);
        if self.remote_state.client.is_none() {
            self.remote_state.client = vorkspace_core::remote::default_connections().ok();
        }
        let client = self.remote_state.client.clone();
        let work = cx
            .background_executor()
            .spawn(async move { Refresh::read(store, id, client, observation, initial) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.finish_remote_snapshot(refresh, submission, initial, result, cx);
            });
        })
        .detach();
    }

    pub(in crate::conversation) fn release_remote_snapshot(&mut self) {
        self.remote_state.observation = SessionObservation::default();
        self.remote_state.refresh_generation = self.remote_state.refresh_generation.wrapping_add(1);
        self.remote_state.refreshing = false;
    }

    pub(super) fn finish_remote_snapshot(
        &mut self,
        refresh: u64,
        submission: u64,
        initial: bool,
        update: Refresh,
        cx: &mut Context<Self>,
    ) {
        if self.remote_state.refresh_generation != refresh {
            return;
        }
        self.remote_state.refreshing = false;
        if self.submission_generation != submission || self.busy || self.stopping {
            return;
        }
        self.loading = false;
        let Refresh {
            observation,
            binding,
            result,
        } = update;
        let Some(binding) = binding else {
            if let Err(error) = result {
                self.remote_connection_error(&error, cx);
            }
            return;
        };
        let pending = !binding.outbox.is_empty()
            || binding
                .rejected
                .as_ref()
                .is_some_and(|rejected| rejected["legacyRecovery"] == true);
        let recovery_changed =
            self.remote_state.pending != pending || self.remote_state.rejected != binding.rejected;
        self.remote_state.pending = pending;
        self.remote_state.rejected = binding.rejected;
        let projection = match result {
            Ok(projection) => projection,
            Err(error) => {
                self.remote_state.observation = observation;
                self.remote_connection_error(&error, cx);
                return;
            }
        };
        if binding.host_revision.is_some_and(|latest| {
            observation
                .revision()
                .is_none_or(|received| received < latest)
        }) {
            // Durable metadata can advance through a concurrent listing.
            // Leave the view intact and retry with a full snapshot.
            if initial {
                self.remote_connection_error(
                    "The host returned an older conversation snapshot. Retry to load the latest version.",
                    cx,
                );
            } else if recovery_changed {
                cx.notify();
            }
            return;
        }
        let Snapshot {
            mut settings,
            descriptor,
            history,
            metadata_changed,
        } = projection;
        let reconnect = !self.remote_state.online || self.remote_state.awaiting_successor;
        self.remote_state.online = true;
        self.remote_state.awaiting_successor = false;
        self.remote_state.error = self
            .remote_state
            .rejected
            .as_ref()
            .and_then(|rejected| rejected["error"].as_str().map(str::to_owned));
        let summary_changed = self.summary != binding.summary;
        self.summary = binding.summary;
        if let Some(descriptor) = descriptor {
            self.remote_state.capabilities = descriptor["capabilities"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect();
        }
        let configuration_changed = settings.revision > self.settings.revision
            || (initial && settings.revision >= self.settings.revision);
        if configuration_changed {
            settings.turns = std::mem::take(&mut self.settings.turns);
            self.settings = settings;
            self.model = self.settings.model.clone();
        }
        let history_changed = history.is_some();
        if let Some(Projection { history, turns }) = history {
            self.settings.turns = turns.clone();
            self.transcript.update(cx, |view, cx| {
                view.turns = turns;
                view.install_prepared_history(history, cx);
            });
        }
        self.loaded = true;
        self.history_unloaded = false;
        if metadata_changed || reconnect {
            self.status = "Ready".into();
        }
        let mut run = None;
        if let Some(snapshot) = observation.snapshot() {
            if metadata_changed {
                self.context = snapshot["session"]
                    .get("context")
                    .cloned()
                    .and_then(|context| serde_json::from_value(context).ok());
            }
            if history_changed {
                self.install_remote_draft(snapshot, cx);
                self.install_remote_plans(snapshot, cx);
            }
            if snapshot["status"] == "running" {
                run = Some(snapshot["runId"].clone());
            }
        }
        self.remote_state.observation = observation;
        if initial {
            self.refresh_catalog(cx);
            self.refresh_git(cx);
        }
        if let Some((row, query)) = self.pending_search.take() {
            self.reveal_search(row, query, cx);
        }
        if let Some(run) = run.filter(|_| !self.busy && !self.stopping) {
            self.settings.remote_action = Some(json!({"type":"attach", "runId":run}));
            self.launch_prompt(String::new(), cx);
        } else {
            self.run_history_actions(cx);
        }
        if initial {
            cx.emit(SessionChanged::Loaded);
        } else if history_changed {
            cx.emit(SessionChanged::History);
        } else if configuration_changed {
            cx.emit(SessionChanged::Configuration);
        } else if summary_changed {
            cx.emit(SessionChanged::Catalog);
        }
        if metadata_changed
            || reconnect
            || recovery_changed
            || summary_changed
            || configuration_changed
        {
            cx.notify();
        }
    }
}
