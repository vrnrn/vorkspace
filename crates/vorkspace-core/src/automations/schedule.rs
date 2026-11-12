//! Calendar scheduling and trigger descriptions.
use super::{legacy_trigger, new_id, now_millis, Automation, AutomationTrigger, AutomationUpsert};
use chrono::{Datelike, Local, TimeZone, Timelike};
use std::collections::HashMap;

pub const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

impl AutomationTrigger {
    pub fn new(kind: &str, event: &str) -> Self {
        Self {
            id: new_id(),
            kind: kind.into(),
            event: event.into(),
            schedule_kind: if matches!(event, "hourly" | "daily" | "weekdays" | "weekly") {
                event.into()
            } else {
                "weekdays".into()
            },
            minute: 0,
            time: "09:00".into(),
            day_of_week: 1,
            repos: Vec::new(),
            repo: String::new(),
            branch: String::new(),
            actor: "anyone".into(),
        }
    }
    pub fn label(&self) -> String {
        if self.kind != "time" {
            return event_label(&self.kind, &self.event);
        }
        match self.schedule_kind.as_str() {
            "hourly" => format!("Hourly at :{:02}", self.minute),
            "daily" => format!("Daily at {}", self.time),
            "weekly" => format!(
                "{} at {}",
                WEEKDAYS.get(self.day_of_week as usize).unwrap_or(&"Weekly"),
                self.time
            ),
            _ => format!("Weekdays at {}", self.time),
        }
    }
    /// Calendar arithmetic in the user's time zone, including daylight-saving changes.
    pub fn next_at(&self, after: i64) -> i64 {
        let Some(start) = Local.timestamp_millis_opt(after).single() else {
            return after.saturating_add(86_400_000);
        };
        if self.schedule_kind == "hourly" {
            // Enumerating actual clock minutes also handles repeated/nonexistent DST hours.
            let mut candidate = (after / 60_000 + 1) * 60_000;
            for _ in 0..180 {
                if Local
                    .timestamp_millis_opt(candidate)
                    .single()
                    .is_some_and(|date| date.minute() == self.minute.clamp(0, 59) as u32)
                {
                    return candidate;
                }
                candidate += 60_000;
            }
            return candidate;
        }
        let (hour, minute) = self
            .time
            .split_once(':')
            .and_then(|(h, m)| Some((h.parse::<u32>().ok()?, m.parse::<u32>().ok()?)))
            .unwrap_or((9, 0));
        for offset in 0..15 {
            let date = start.date_naive() + chrono::Duration::days(offset);
            let weekday = date.weekday().num_days_from_sunday();
            if self.schedule_kind == "weekdays" && (weekday == 0 || weekday == 6) {
                continue;
            }
            if self.schedule_kind == "weekly" && weekday != self.day_of_week.clamp(0, 6) as u32 {
                continue;
            }
            if let Some(naive) = date.and_hms_opt(hour.min(23), minute.min(59), 0) {
                // A repeated clock time fires once. For a spring-forward gap use the first
                // valid clock minute after the requested wall time, as Date.setHours does.
                for gap in 0..=180 {
                    let target = naive + chrono::Duration::minutes(gap);
                    if let Some(candidate) = Local.from_local_datetime(&target).earliest() {
                        let at = candidate.timestamp_millis();
                        if at > after {
                            return at;
                        }
                        break;
                    }
                }
            }
        }
        after.saturating_add(7 * 86_400_000)
    }
}

pub fn event_label(kind: &str, event: &str) -> String {
    match (kind, event) {
        ("jira", "issue_created") => "Issue appeared",
        (_, "draft_opened") => "Draft opened",
        (_, "pull_request_opened") => "Pull request opened",
        (_, "issue_opened") => "Issue opened",
        (_, "merge_request_opened") => "Merge request opened",
        (_, "issue_created") => "Issue created",
        (_, "pull_request_appeared") => "Pull request appeared",
        (_, "work_item_appeared") => "Work item appeared",
        (_, "pull_request_merged") => "Pull request merged",
        (_, "pull_request_review_requested") => "Review requested",
        (_, "issue_assigned") => "Issue assigned",
        (_, "check_failed") => "Check failed",
        _ => event,
    }
    .to_owned()
}

pub fn next_run_at(triggers: &[AutomationTrigger], after: i64) -> i64 {
    triggers
        .iter()
        .filter(|t| t.kind == "time")
        .map(|t| t.next_at(after))
        .min()
        .unwrap_or_else(|| after.saturating_add(365 * 86_400_000))
}

impl Automation {
    pub(crate) fn triggers(&self) -> Vec<AutomationTrigger> {
        self.triggers
            .clone()
            .unwrap_or_else(|| vec![legacy_trigger(self)])
    }
    pub fn trigger_label(&self) -> String {
        let triggers = self.triggers();
        match triggers.len() {
            0 => "Manual only".into(),
            1 => triggers[0].label(),
            count => format!("{} + {} more", triggers[0].label(), count - 1),
        }
    }
}

impl AutomationUpsert {
    pub fn new(cwd: String, harness: String, model: String) -> Self {
        Self {
            id: new_id(),
            name: String::new(),
            prompt: String::new(),
            harness,
            model,
            model_settings: HashMap::new(),
            cwd,
            workspace_mode: "worktree".into(),
            worktree_cwd: String::new(),
            session_folder_id: String::new(),
            reuse_session: false,
            runtime_mode: "auto".into(),
            trigger_kind: "time".into(),
            trigger_event: String::new(),
            schedule_kind: "weekdays".into(),
            minute: 0,
            time: "09:00".into(),
            day_of_week: 1,
            triggers: Some(Vec::new()),
            missed_run_grace_minutes: 720,
            enabled: true,
            next_run_at: now_millis().saturating_add(365 * 86_400_000),
        }
    }
    pub fn from_automation(automation: &Automation) -> Self {
        Self {
            id: automation.id.clone(),
            name: automation.name.clone(),
            prompt: automation.prompt.clone(),
            harness: automation.harness.clone(),
            model: automation.model.clone(),
            model_settings: automation.model_settings.clone(),
            cwd: automation.cwd.clone(),
            workspace_mode: automation.workspace_mode.clone(),
            worktree_cwd: automation.worktree_cwd.clone(),
            session_folder_id: automation.session_folder_id.clone(),
            reuse_session: automation.reuse_session,
            runtime_mode: automation.runtime_mode.clone(),
            trigger_kind: automation.trigger_kind.clone(),
            trigger_event: automation.trigger_event.clone(),
            schedule_kind: automation.schedule_kind.clone(),
            minute: automation.minute,
            time: automation.time.clone(),
            day_of_week: automation.day_of_week,
            triggers: Some(automation.triggers()),
            missed_run_grace_minutes: automation.missed_run_grace_minutes,
            enabled: automation.enabled,
            next_run_at: automation.next_run_at,
        }
    }
    pub fn sync_triggers(&mut self, now: i64) {
        let triggers = self.triggers.get_or_insert_with(Vec::new);
        if let Some(primary) = triggers
            .iter()
            .find(|t| t.kind == "time")
            .or_else(|| triggers.first())
        {
            self.trigger_kind = primary.kind.clone();
            self.trigger_event = primary.event.clone();
            self.schedule_kind = primary.schedule_kind.clone();
            self.minute = primary.minute;
            self.time = primary.time.clone();
            self.day_of_week = primary.day_of_week;
        } else {
            self.trigger_kind = "time".into();
            self.trigger_event.clear();
        }
        self.next_run_at = next_run_at(triggers, now);
    }
}
