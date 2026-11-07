//! A manual compaction is a local provider operation without a user turn.
use super::*;
use std::collections::HashMap;
use std::path::PathBuf;
use vorkspace_core::{
    agent::TurnInfo,
    context::Usage,
    tool::{ToolCall, ToolState},
};

pub const ATTEMPT_PREFIX: &str = "manual-compaction-";

pub struct Started {
    pub rows: Vec<Row>,
    pub turns: HashMap<usize, TurnInfo>,
}

pub struct Finished {
    pub result: Result<(), String>,
    pub row: Option<(usize, Row)>,
    pub context: Option<Usage>,
    pub commands: Option<Vec<provider::NativeCommand>>,
}

pub fn run(
    summary: Session,
    settings: Settings,
    store: Database,
    cancel: Arc<AtomicBool>,
    started: impl FnOnce(Started),
) -> Finished {
    run_using(
        summary,
        settings,
        store,
        cancel,
        started,
        provider::resolve,
        |request, cancel, emit| provider::compact(request, cancel, emit),
    )
}

pub(super) fn run_using(
    summary: Session,
    mut settings: Settings,
    store: Database,
    cancel: Arc<AtomicBool>,
    started: impl FnOnce(Started),
    resolve: impl FnOnce(Kind) -> Option<PathBuf>,
    compact: impl FnOnce(
        Request,
        Arc<AtomicBool>,
        &mut dyn FnMut(Vec<Event>) -> Result<(), String>,
    ) -> Result<(), String>,
) -> Finished {
    let id = summary.id;
    let account = settings.account.id.clone();
    let cwd = settings
        .worktree_cwd
        .clone()
        .unwrap_or_else(|| summary.cwd.clone());
    let attempt = format!("{ATTEMPT_PREFIX}{}", vorkspace_core::bot::new_id());
    let mut tool = ToolCall::new(&attempt, "contextCompaction");
    let mut submissions = submissions::Submissions::new(store.clone(), id);
    let mut saved = None;
    let mut context = Usage::default();
    let mut commands = None;
    let result = (|| {
        let kind = Kind::from_id(&summary.provider);
        if !provider::supports_compact(kind) {
            return Err(format!(
                "Context compaction is not supported by the native {} transport.",
                kind.label()
            ));
        }
        let thread = summary
            .provider_id
            .as_deref()
            .ok_or("Send a message before compacting this chat.")?;
        {
            let owner = locked(&store)?;
            if owner.remote_session(id)?.is_some() {
                return Err("Remote transcripts are owned by the host".into());
            }
            if let Some(reason) = owner.edit_recovery_reason(id)? {
                return Err(reason);
            }
            if account != vorkspace_core::accounts::DEFAULT {
                if !owner.provider_accounts()?.exists(kind, &account) {
                    return Err(
                        "This provider account was removed. Choose another account to continue."
                            .into(),
                    );
                }
                if settings.account.root.is_none() {
                    settings.account.root = owner.data_dir().map(PathBuf::from);
                }
            }
        }
        let history = native_history::before_turn(&store, &summary, &settings, &cancel)?;
        if cancel.load(Ordering::Relaxed) {
            return Err("Stopped".into());
        }
        submissions.reserve_compaction(&attempt, history.as_ref())?;
        let history = {
            let mut owner = locked(&store)?;
            if !owner.compaction_binding_current(&summary, &account, &cwd)?
                || vorkspace_core::worktree_management::is_being_removed(&cwd)
            {
                return Err(
                    "The conversation changed before compaction. Retry with its current settings."
                        .into(),
                );
            }
            let (mut history, mut writer) = owner.load_turn_history(id)?;
            let turns = owner.agent_settings(id)?.unwrap_or_default().turns;
            context = owner.context_usage(id)?.unwrap_or_default();
            let at = history.rows.len();
            let row = Row {
                role: "tool".into(),
                text: serde_json::to_string(&tool).map_err(|error| error.to_string())?,
                first: true,
                sent_at: Some(vorkspace_core::time::now_ms()),
            };
            if !owner.write_turn_rows(id, at, std::slice::from_ref(&row), None, writer.as_mut())? {
                return Err("The saved conversation changed before compaction.".into());
            }
            history.rows.push(row.clone());
            saved = Some((at, row, writer));
            Started {
                rows: history.rows,
                turns,
            }
        };
        started(history);
        let binary = resolve(kind).ok_or_else(|| format!("{} CLI not found.", kind.label()))?;
        if cancel.load(Ordering::Relaxed) {
            return Err("Stopped".into());
        }
        if !locked(&store)?.compaction_binding_current(&summary, &account, &cwd)?
            || vorkspace_core::worktree_management::is_being_removed(&cwd)
        {
            return Err(
                "The conversation changed before compaction. Retry with its current settings."
                    .into(),
            );
        }
        submissions.dispatch_initial()?;
        let request = Request {
            kind,
            binary,
            cwd: PathBuf::from(&cwd),
            prompt: String::new(),
            provider_id: Some(thread.into()),
            model: settings.model.clone(),
            settings,
        };
        vorkspace_core::process::owned_by(id, || {
            compact(request, cancel.clone(), &mut |events| {
                for event in events {
                    match event {
                        Event::SessionId(id)
                            if matches!(kind, Kind::Codex | Kind::Claude) && id != thread =>
                        {
                            return Err("The provider changed sessions during compaction.".into());
                        }
                        Event::Context { used, window } => context.merge(used, window),
                        Event::Commands(discovered) => commands = Some(discovered),
                        _ => {}
                    }
                }
                Ok(())
            })
        })
    })();

    let mut row = None;
    let mut context_saved = false;
    let save = (|| {
        let Some((at, mut current_row, mut writer)) = saved else {
            return Ok(());
        };
        tool.state = match &result {
            Ok(()) => ToolState::Completed,
            Err(error) if error == "Stopped" || cancel.load(Ordering::Relaxed) => {
                ToolState::Cancelled
            }
            Err(_) => ToolState::Failed,
        };
        tool.output = result.as_ref().err().cloned().unwrap_or_default();
        current_row.text = serde_json::to_string(&tool).map_err(|error| error.to_string())?;
        let mut owner = locked(&store)?;
        if !owner.write_turn_rows(
            id,
            at,
            std::slice::from_ref(&current_row),
            None,
            writer.as_mut(),
        )? {
            return Err("The saved conversation changed during compaction.".into());
        }
        row = Some((at, current_row));
        if result.is_ok() {
            context_saved =
                owner.save_compaction_context_usage(&summary, &account, &cwd, &context)?;
            if !context_saved {
                return Err("The conversation changed during compaction.".into());
            }
        }
        Ok(())
    })();
    let confirmed = result.is_ok() && row.is_some();
    let result = combine(result, save);
    let receipt = submissions.finish(&result, confirmed);
    Finished {
        result: combine(result, receipt),
        row,
        context: context_saved.then_some(context),
        commands: commands.filter(|_| context_saved),
    }
}

fn combine(result: Result<(), String>, save: Result<(), String>) -> Result<(), String> {
    match (result, save) {
        (Err(error), Err(save)) => Err(format!("{error} · Saving compaction failed: {save}")),
        (result, save) => result.and(save),
    }
}
