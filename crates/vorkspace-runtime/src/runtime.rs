use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use vorkspace_core::agent::{Control, Settings};
use vorkspace_core::{
    provider::{self, Event, Kind, Request},
    store::{ComposerOrigin, Session, Store},
    transcript::{Row, Transcript},
};
pub mod card_persistence;
#[cfg(test)]
mod compaction_tests;
#[cfg(test)]
mod follow_up_tests;
mod handoff;
pub mod manual_compaction;
pub mod native_history;
mod persistence;
mod running_turn;
mod submissions;
pub mod turns;
mod updates;
pub use updates::{
    HistoryRecovery, ProviderBinding, ProviderConfiguration, TranscriptPatch, TurnOutcome,
    TurnSettlement, TurnTransition, Update,
};
#[cfg(test)]
mod ticket_context_tests;
mod visual_discovery;
#[cfg(test)]
mod visual_tests;
use persistence::PendingSave;
pub type Database = Arc<Mutex<Store>>;
pub struct TurnInput {
    pub summary: Session,
    pub prompt: String,
    pub settings: Settings,
    pub composer_origin: Option<ComposerOrigin>,
}

fn publish(
    tx: &async_channel::Sender<Update>,
    cancel: &AtomicBool,
    mut update: Update,
) -> Result<(), String> {
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Stopped".into());
        }
        match tx.try_send(update) {
            Ok(()) => return Ok(()),
            Err(async_channel::TrySendError::Closed(_)) => return Err("Window closed".into()),
            Err(async_channel::TrySendError::Full(value)) => update = value,
        }
        std::thread::sleep(Duration::from_millis(4));
    }
}
pub fn locked(store: &Database) -> Result<std::sync::MutexGuard<'_, Store>, String> {
    store
        .lock()
        .map_err(|_| "Native session store unavailable".into())
}

fn run_turn(
    input: TurnInput,
    controls: std::sync::mpsc::Receiver<Control>,
    store: Database,
    cancel: Arc<AtomicBool>,
    tx: async_channel::Sender<Update>,
) {
    run_turn_using(input, controls, store, cancel, tx, provider::resolve);
}

use running_turn::run_turn_using;

/// What a turn's prompt needs to know about the conversation so far.
struct TurnHistory<'a> {
    summary: &'a Session,
    earlier: &'a [vorkspace_core::transcript::Row],
}

/// Records context delivery against the compaction visible BEFORE the turn.
/// A compaction during the turn must leave the next turn due for a refresh.
struct BotContextReceipt {
    hash: String,
    compaction: Option<String>,
    native_compaction: Option<String>,
}

impl BotContextReceipt {
    fn new(context: &str, earlier: &[Row]) -> Self {
        let compaction = earlier.iter().rev().find_map(|row| {
            if row.role != "tool" || !row.text.contains("\"contextCompaction\"") {
                return None;
            }
            let tool = serde_json::from_str::<vorkspace_core::tool::ToolCall>(&row.text).ok()?;
            // Failed, cancelled and interrupted attempts also refresh context:
            // their local outcome cannot prove the provider retained everything.
            tool.is_compaction().then_some(tool.id)
        });
        Self {
            hash: vorkspace_core::bot::files::content_hash(context),
            compaction,
            native_compaction: None,
        }
    }

    fn encode(&self, provider_id: Option<&str>) -> String {
        serde_json::json!({
            "provider": provider_id,
            "hash": self.hash,
            "compaction": self.compaction,
            "nativeCompaction": self.native_compaction,
        })
        .to_string()
    }
}

/// Prefixes a turn with the Bot or minion context the provider hasn't seen yet.
/// Providers own compaction; app-owned instructions and memory are reintroduced
/// after a recorded compaction without discarding the provider's session.
fn turn_prompt(
    prompt: String,
    settings: &Settings,
    history: TurnHistory<'_>,
    provider_id: Option<&str>,
    bot_context: &mut Option<BotContextReceipt>,
    store: &Database,
) -> Result<String, String> {
    let added = if let Some(bot) = &settings.bot {
        let files = vorkspace_core::bot::load_files(&bot.root, &bot.bot)?;
        Some(vorkspace_core::bot::context(
            &bot.bot,
            &files,
            &bot.executable,
        ))
    } else {
        None
    };
    let prompt = if provider_id.is_none()
        && !history.earlier.is_empty()
        && settings.handoff.is_none()
        && settings.bot.as_ref().is_some_and(|b| {
            b.bot.compaction_mode == vorkspace_core::bot::CompactionMode::SessionRollover
        }) {
        format!(
            "{}\n\n{prompt}",
            vorkspace_core::bot::rotation_brief(history.earlier)
        )
    } else {
        continuing_prompt(
            settings.handoff.as_ref(),
            provider_id.is_none(),
            history.earlier,
            prompt,
        )
    };
    let Some(added) = added else {
        return Ok(prompt);
    };
    let mut receipt = BotContextReceipt::new(&added, history.earlier);
    if let Some(bot) = settings.bot.as_ref().filter(|_| !settings.text_only) {
        receipt.native_compaction = locked(store)?
            .native_history_request(history.summary.id, &bot.root)?
            .and_then(|r| r.cursor)
            .and_then(|c| c.compaction);
    }
    let previous = locked(store)?.setting(&format!("bot-context:{}", history.summary.id))?;
    let refresh =
        provider_id.is_none() || previous.as_deref() != Some(receipt.encode(provider_id).as_str());
    *bot_context = Some(receipt);
    if refresh {
        Ok(format!("{added}\n\n{prompt}"))
    } else {
        Ok(prompt)
    }
}

/// checkpointEdits.ts's sessionEditPaths: files named by successful edit tools.
/// A turn's prompt with whatever the provider's thread lacks: a handoff's
/// brief, or a bounded recap of `prior` when a thread starts fresh mid-way,
/// such as after a first turn failed before the provider assigned an ID.
fn continuing_prompt(
    handoff: Option<&vorkspace_core::agent::Handoff>,
    fresh_thread: bool,
    prior: &[Row],
    prompt: String,
) -> String {
    if let Some(handoff) = handoff {
        return vorkspace_core::response::wrap_handoff_prompt(
            &handoff.from,
            &handoff.brief,
            &prompt,
        );
    }
    if !fresh_thread || prior.is_empty() {
        return prompt;
    }
    let mut history = String::new();
    for row in prior
        .iter()
        .rev()
        .filter(|r| r.is_prompt() || r.role == "assistant")
    {
        if history.len() + row.text.len() > 32_000 {
            break;
        }
        history = format!("{}: {}\n\n{history}", row.role, row.text);
    }
    format!(
        "Continue this conversation, using the prior messages as context.\n\n{history}\nCurrent user message:\n{prompt}"
    )
}

fn edited_paths(rows: &[Row]) -> Vec<String> {
    let mut paths = Vec::new();
    for row in rows.iter().filter(|r| r.role == "tool") {
        let Ok(tool) = serde_json::from_str::<vorkspace_core::tool::ToolCall>(&row.text) else {
            continue;
        };
        if tool.state != vorkspace_core::tool::ToolState::Completed {
            continue;
        }
        let (action, path) = tool.summary();
        if matches!(
            action.as_str(),
            "Edit" | "Write" | "MultiEdit" | "NotebookEdit"
        ) && !path.is_empty()
            && !paths.contains(&path)
        {
            paths.push(path);
        }
    }
    paths
}

fn shlex_quote(text: &str) -> String {
    format!(
        "\"{}\"",
        text.replace('\\', "\\\\")
            .replace('\"', "\\\"")
            .replace('$', "\\$")
            .replace('`', "\\`")
    )
}

#[cfg(test)]
mod continuing_prompt_tests {
    use super::*;

    fn row(role: &str, text: &str) -> Row {
        Row {
            role: role.into(),
            text: text.into(),
            first: false,
            sent_at: None,
        }
    }

    #[test]
    fn a_fresh_thread_recaps_the_saved_task_and_stays_bounded() {
        // A first turn failed before the provider assigned a thread.
        let prior = [
            row("user", "Scout the repository layout"),
            row(
                "bot-system",
                "The delegated scout finished; inspect its saved report",
            ),
            row("tool", "ignored"),
            row("assistant", "The provider was unavailable"),
        ];
        let prompt = continuing_prompt(None, true, &prior, "try again".into());
        assert!(prompt.contains("user: Scout the repository layout"));
        assert!(
            prompt.contains("bot-system: The delegated scout finished; inspect its saved report")
        );
        assert!(prompt.contains("assistant: The provider was unavailable"));
        assert!(prompt.ends_with("Current user message:\ntry again"));
        assert!(!prompt.contains("ignored"));
        // The newest messages are the ones kept.
        let long = [row("user", &"old ".repeat(9_000)), row("user", "recent")];
        let prompt = continuing_prompt(None, true, &long, "next".into());
        assert!(prompt.contains("recent"));
        assert!(!prompt.contains("old old"));
    }

    #[test]
    fn nothing_is_added_when_the_thread_already_has_it() {
        let prior = [row("user", "Earlier")];
        assert_eq!(
            continuing_prompt(None, false, &prior, "next".into()),
            "next"
        );
        assert_eq!(continuing_prompt(None, true, &[], "first".into()), "first");
    }

    #[test]
    fn a_handoff_takes_the_place_of_the_recap() {
        let handoff = vorkspace_core::agent::Handoff {
            from: "codex".into(),
            brief: "The brief".into(),
        };
        let prior = [row("user", "Earlier")];
        let prompt = continuing_prompt(Some(&handoff), true, &prior, "next".into());
        assert!(prompt.contains("The brief"));
        assert!(!prompt.contains("Earlier"));
    }
}

#[cfg(test)]
mod unsupported_history_tests {
    use super::*;

    #[test]
    fn retired_history_rejects_a_turn_before_resolution_or_durable_mutation() {
        for provider in [
            "cursor", "grok", "opencode", "fx", "hermes", "devin", "unknown",
        ] {
            let mut store = Store::test(std::path::Path::new(":memory:")).unwrap();
            let session = store.create("codex", "/history").unwrap();
            store.change_provider(session.id, provider).unwrap();
            store.provider_id(session.id, "original-thread").unwrap();
            let settings = Settings {
                revision: 7,
                model: Some("original-model".into()),
                ..Default::default()
            };
            store.save_agent_settings(session.id, &settings).unwrap();
            let rows = vec![Row {
                role: "assistant".into(),
                text: "Retained historical work".into(),
                first: true,
                sent_at: Some(123),
            }];
            store.write_rows(session.id, 0, &rows).unwrap();
            let saved_settings =
                serde_json::to_value(store.agent_settings(session.id).unwrap().unwrap()).unwrap();
            let summary = store.sessions().unwrap().remove(0);
            let store = Arc::new(Mutex::new(store));
            let (_, controls) = std::sync::mpsc::channel();
            let (tx, rx) = async_channel::bounded(8);
            run_turn_using(
                TurnInput {
                    composer_origin: None,
                    summary: summary.clone(),
                    prompt: "Continue".into(),
                    settings: settings.clone(),
                },
                controls,
                store.clone(),
                Arc::new(AtomicBool::new(false)),
                tx,
                |_| panic!("a retired provider must never reach CLI resolution"),
            );
            let update = rx.recv_blocking().unwrap();
            assert!(update.finished());
            assert!(update.status.contains("no longer supported"));
            assert!(update.transcript.rows().is_empty());
            let store = locked(&store).unwrap();
            let after = store.sessions().unwrap().remove(0);
            assert_eq!(after.provider, provider);
            assert_eq!(after.provider_id, Some("original-thread".into()));
            assert_eq!(
                serde_json::to_value(store.load(session.id).unwrap().rows).unwrap(),
                serde_json::to_value(rows).unwrap()
            );
            assert_eq!(
                serde_json::to_value(store.agent_settings(session.id).unwrap().unwrap()).unwrap(),
                saved_settings
            );
            assert!(store.local_submissions(session.id).unwrap().is_empty());
        }
    }
}
