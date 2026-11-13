//! Persisted automations, scheduling, event deduplication, and run history.
mod schedule;
pub(crate) mod schema;
mod service;
pub use schedule::{event_label, next_run_at, WEEKDAYS};
pub use service::{DefinitionRevision, Service};

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

fn now_millis() -> i64 {
    crate::time::now_ms().min(i64::MAX as u64) as i64
}

pub fn new_id() -> String {
    format!("{:032x}", rand::random::<u128>())
}

fn validate_id(id: &str, kind: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 200
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':'))
    {
        return Err(format!("Invalid {kind} id."));
    }
    Ok(())
}

const MAX_NAME: usize = 200;
const MAX_PROMPT: usize = 1_000_000;
const MAX_TRIGGERS: usize = 20;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Automation {
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub harness: String,
    pub model: String,
    #[serde(default)]
    pub model_settings: HashMap<String, String>,
    pub cwd: String,
    pub workspace_mode: String,
    #[serde(default)]
    pub worktree_cwd: String,
    #[serde(default)]
    pub session_folder_id: String,
    pub reuse_session: bool,
    pub runtime_mode: String,
    #[serde(default = "default_trigger_kind")]
    pub trigger_kind: String,
    #[serde(default)]
    pub trigger_event: String,
    pub schedule_kind: String,
    pub minute: i64,
    pub time: String,
    pub day_of_week: i64,
    #[serde(default)]
    pub triggers: Option<Vec<AutomationTrigger>>,
    pub missed_run_grace_minutes: i64,
    pub enabled: bool,
    pub next_run_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_session_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationUpsert {
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub harness: String,
    pub model: String,
    #[serde(default)]
    pub model_settings: HashMap<String, String>,
    pub cwd: String,
    pub workspace_mode: String,
    #[serde(default)]
    pub worktree_cwd: String,
    #[serde(default)]
    pub session_folder_id: String,
    #[serde(default)]
    pub reuse_session: bool,
    pub runtime_mode: String,
    #[serde(default = "default_trigger_kind")]
    pub trigger_kind: String,
    #[serde(default)]
    pub trigger_event: String,
    pub schedule_kind: String,
    pub minute: i64,
    pub time: String,
    pub day_of_week: i64,
    #[serde(default)]
    pub triggers: Option<Vec<AutomationTrigger>>,
    pub missed_run_grace_minutes: i64,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub next_run_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationRun {
    pub id: String,
    pub automation_id: String,
    pub trigger: String,
    pub scheduled_for: i64,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationTrigger {
    #[serde(default)]
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub event: String,
    #[serde(default = "default_schedule_kind")]
    pub schedule_kind: String,
    #[serde(default)]
    pub minute: i64,
    #[serde(default = "default_time")]
    pub time: String,
    #[serde(default = "default_day_of_week")]
    pub day_of_week: i64,
    #[serde(default)]
    pub repos: Vec<String>,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default = "default_actor")]
    pub actor: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DueAutomationRun {
    pub automation: Automation,
    pub run: AutomationRun,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AutomationEventClaim {
    pub event_key: String,
    pub event_kind: String,
    pub event: String,
    pub scheduled_for: i64,
    pub prompt: String,
}

fn default_true() -> bool {
    true
}

fn default_trigger_kind() -> String {
    "time".into()
}

fn default_schedule_kind() -> String {
    "weekdays".into()
}

fn default_time() -> String {
    "09:00".into()
}

fn default_day_of_week() -> i64 {
    1
}

fn default_actor() -> String {
    "anyone".into()
}

fn validate_upsert(input: &AutomationUpsert, now: i64) -> Result<(), String> {
    validate_id(&input.id, "automation")?;
    if input.name.trim().is_empty() || input.name.len() > MAX_NAME {
        return Err("Automation name is required and must be under 200 characters.".into());
    }
    if input.prompt.trim().is_empty() || input.prompt.len() > MAX_PROMPT {
        return Err("Automation prompt is required.".into());
    }
    if input.cwd.trim().is_empty() {
        return Err("Choose a project for this automation.".into());
    }
    if !matches!(
        input.workspace_mode.as_str(),
        "current" | "worktree" | "existing"
    ) {
        return Err("Invalid automation workspace mode.".into());
    }
    if input.workspace_mode == "existing" {
        let path = input.worktree_cwd.trim();
        if path.is_empty() || path.len() > 1_000 {
            return Err("Choose an existing worktree for this automation.".into());
        }
    }
    if input.session_folder_id.len() > 80 {
        return Err("Invalid automation session folder.".into());
    }
    if !matches!(
        input.runtime_mode.as_str(),
        "supervised" | "auto-accept-edits" | "auto" | "full-access"
    ) {
        return Err("Invalid automation run mode.".into());
    }
    if !matches!(
        input.trigger_kind.as_str(),
        "time" | "github" | "gitlab" | "jira"
    ) {
        return Err("Invalid automation trigger.".into());
    }
    if input.trigger_event.len() > 200 {
        return Err("Invalid automation trigger event.".into());
    }
    if !matches!(
        input.schedule_kind.as_str(),
        "hourly" | "daily" | "weekdays" | "weekly"
    ) {
        return Err("Invalid automation schedule.".into());
    }
    if !(0..=59).contains(&input.minute)
        || !(0..=6).contains(&input.day_of_week)
        || !valid_time(&input.time)
    {
        return Err("Invalid automation time.".into());
    }
    if !(0..=43_200).contains(&input.missed_run_grace_minutes) {
        return Err("Invalid missed-run grace period.".into());
    }
    if input.next_run_at <= now {
        return Err("The next automation run must be in the future.".into());
    }
    if let Some(triggers) = &input.triggers {
        if triggers.len() > MAX_TRIGGERS {
            return Err("Too many automation triggers.".into());
        }
        for trigger in triggers {
            validate_trigger(trigger)?;
        }
    }
    Ok(())
}

fn validate_trigger(trigger: &AutomationTrigger) -> Result<(), String> {
    if !matches!(trigger.kind.as_str(), "time" | "github" | "gitlab" | "jira") {
        return Err("Invalid automation trigger.".into());
    }
    if trigger.event.len() > 200
        || trigger.repo.len() > 400
        || trigger.branch.len() > 400
        || trigger.actor.len() > 200
        || trigger.repos.len() > 50
        || trigger.repos.iter().any(|repo| repo.len() > 400)
    {
        return Err("Invalid automation trigger event.".into());
    }
    if !matches!(
        trigger.schedule_kind.as_str(),
        "hourly" | "daily" | "weekdays" | "weekly"
    ) {
        return Err("Invalid automation schedule.".into());
    }
    if !(0..=59).contains(&trigger.minute)
        || !(0..=6).contains(&trigger.day_of_week)
        || !valid_time(&trigger.time)
    {
        return Err("Invalid automation time.".into());
    }
    Ok(())
}

fn has_time_trigger(automation: &Automation) -> bool {
    match &automation.triggers {
        Some(triggers) => triggers.iter().any(|trigger| trigger.kind == "time"),
        None => false,
    }
}

fn normalize_triggers(input: &AutomationUpsert) -> Vec<AutomationTrigger> {
    let mut triggers = input.triggers.clone().unwrap_or_else(|| {
        vec![trigger_from_fields(
            &input.id,
            &input.trigger_kind,
            &input.trigger_event,
            &input.schedule_kind,
            input.minute,
            &input.time,
            input.day_of_week,
        )]
    });
    for trigger in &mut triggers {
        if trigger.id.trim().is_empty() {
            trigger.id = new_id();
        }
        trigger.event = trigger.event.trim().to_string();
        trigger.repo = trigger.repo.trim().to_string();
        trigger.branch = trigger.branch.trim().to_string();
        trigger.actor = trigger.actor.trim().to_string();
        if trigger.actor.is_empty() {
            trigger.actor = default_actor();
        }
        trigger.repos = trigger
            .repos
            .iter()
            .map(|repo| repo.trim().to_string())
            .filter(|repo| !repo.is_empty())
            .collect();
    }
    triggers
}

fn trigger_from_fields(
    id: &str,
    kind: &str,
    event: &str,
    schedule_kind: &str,
    minute: i64,
    time: &str,
    day_of_week: i64,
) -> AutomationTrigger {
    AutomationTrigger {
        id: format!("{id}:legacy"),
        kind: kind.to_string(),
        event: if event.is_empty() {
            schedule_kind.to_string()
        } else {
            event.to_string()
        },
        schedule_kind: schedule_kind.to_string(),
        minute,
        time: time.to_string(),
        day_of_week,
        repos: Vec::new(),
        repo: String::new(),
        branch: String::new(),
        actor: default_actor(),
    }
}

fn valid_time(value: &str) -> bool {
    let Some((hour, minute)) = value.split_once(':') else {
        return false;
    };
    hour.len() == 2
        && minute.len() == 2
        && hour.parse::<u8>().is_ok_and(|value| value < 24)
        && minute.parse::<u8>().is_ok_and(|value| value < 60)
}

/// Matches the original Inbox appeared-event filters before using the durable
/// claim ledger. Polling, restarting and overlapping refreshes cannot rerun an item.
pub fn claim_inbox_events(
    service: &mut Service,
    items: &[crate::inbox::Item],
    now: i64,
) -> Result<Vec<DueAutomationRun>, String> {
    claim_inbox_events_retry(service, items, now).map(|(runs, _)| runs)
}

/// A failed match leaves its Inbox item retryable while successful claims are
/// still returned for dispatch. The ledger makes retries of partial batches safe.
pub fn claim_inbox_events_retry(
    service: &mut Service,
    items: &[crate::inbox::Item],
    now: i64,
) -> Result<(Vec<DueAutomationRun>, Vec<crate::inbox::Item>), String> {
    let automations = service.list()?;
    let mut claimed = Vec::new();
    let mut retry = Vec::new();
    for item in items {
        let event = match (item.provider.as_str(), item.kind.as_str(), item.draft) {
            ("github", "pr", true) => "draft_opened",
            ("github", "pr", false) => "pull_request_opened",
            ("github" | "gitlab", "issue", _) => "issue_opened",
            ("gitlab", "pr", _) => "merge_request_opened",
            ("jira", _, _) => "issue_created",
            _ => continue,
        };
        let identity = if crate::inbox::is_tracker(&item.provider) {
            let id = if !item.id.is_empty() {
                item.id.clone()
            } else if !item.identifier.is_empty() {
                item.identifier.clone()
            } else {
                item.number.to_string()
            };
            format!("{}:issue:{id}", item.provider)
        } else {
            format!(
                "{}:{}:{}:{}",
                item.provider, item.kind, item.repo, item.number
            )
        };
        let event_key = identity
            .trim()
            .to_lowercase()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-' | '.' | '/') {
                    c
                } else {
                    '_'
                }
            })
            .take(400)
            .collect::<String>();
        for automation in &automations {
            if !automation.enabled {
                continue;
            }
            let tracker_without_project =
                crate::inbox::is_tracker(&item.provider) && item.project_path.trim().is_empty();
            if !tracker_without_project && !item.belongs_to_project(&automation.cwd) {
                continue;
            }
            let trigger = automation.triggers().into_iter().find(|trigger| {
                if trigger.kind != item.provider || trigger.event != event {
                    return false;
                }
                let actor = trigger.actor.trim();
                if !actor.is_empty() && !actor.eq_ignore_ascii_case("anyone") {
                    return false;
                }
                let repos: Vec<_> = trigger
                    .repos
                    .iter()
                    .chain(std::iter::once(&trigger.repo))
                    .map(|repo| repo.trim())
                    .filter(|repo| !repo.is_empty())
                    .collect();
                repos.is_empty()
                    || repos
                        .iter()
                        .any(|repo| repo.eq_ignore_ascii_case(item.repo.trim()))
            });
            let Some(trigger) = trigger else {
                continue;
            };
            let provider = crate::inbox::provider_label(&item.provider);
            let kind = if crate::inbox::is_tracker(&item.provider) || item.kind != "pr" {
                "issue"
            } else if item.provider == "gitlab" {
                "merge request"
            } else {
                "pull request"
            };
            let reference = if crate::inbox::is_tracker(&item.provider) {
                if item.identifier.trim().is_empty() {
                    format!("{provider} #{}", item.number)
                } else {
                    item.identifier.trim().to_string()
                }
            } else {
                format!("#{}", item.number)
            };
            let title = if item.title.trim().is_empty() {
                if crate::inbox::is_tracker(&item.provider) {
                    reference.clone()
                } else {
                    format!("{provider} {kind} #{}", item.number)
                }
            } else {
                item.title.trim().to_string()
            };
            let mut prompt = format!(
                "{}\n\nWork on this {provider} {kind}:\n\n{reference} {title}",
                automation.prompt.trim()
            );
            if !item.url.trim().is_empty() {
                prompt.push('\n');
                prompt.push_str(item.url.trim());
            }
            let created = crate::inbox::timestamp_ms(&item.created_at);
            let occurred_at = if created != 0 {
                created
            } else {
                item.updated_ms()
            };
            match service.claim_event(
                &automation.id,
                AutomationEventClaim {
                    event_key: event_key.clone(),
                    event_kind: trigger.kind,
                    event: trigger.event,
                    scheduled_for: if occurred_at == 0 { now } else { occurred_at },
                    prompt,
                },
                now,
            ) {
                Ok(Some(run)) => claimed.push(run),
                Ok(None) => {}
                Err(_) => {
                    if !retry
                        .iter()
                        .any(|existing: &crate::inbox::Item| existing == item)
                    {
                        retry.push(item.clone());
                    }
                }
            }
        }
    }
    Ok((claimed, retry))
}
