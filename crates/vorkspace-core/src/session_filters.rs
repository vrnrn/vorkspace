//! SessionFiltersMenu/sessionFilters.ts, shared by native sidebar consumers.
use crate::{kind::Kind, store::Session};
use chrono::{Local, TimeZone};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimeFilter {
    #[default]
    #[serde(rename = "all")]
    All,
    #[serde(rename = "today")]
    Today,
    #[serde(rename = "7d")]
    Week,
    #[serde(rename = "30d")]
    Month,
}
impl TimeFilter {
    pub const OPTIONS: [(Self, &'static str, &'static str); 4] = [
        (Self::All, "all", "All time"),
        (Self::Today, "today", "Today"),
        (Self::Week, "7d", "Last 7 days"),
        (Self::Month, "30d", "Last 30 days"),
    ];
    pub fn start(self, now: u64) -> u64 {
        match self {
            Self::All => 0,
            Self::Week => now.saturating_sub(7 * 86_400_000),
            Self::Month => now.saturating_sub(30 * 86_400_000),
            Self::Today => Local
                .timestamp_millis_opt(now.min(i64::MAX as u64) as i64)
                .single()
                .and_then(|date| {
                    let day = date.date_naive();
                    // Some time zones advance past midnight. Use the first
                    // valid local minute of the day, including ambiguous dates.
                    (0..1440).find_map(|minute| {
                        Local
                            .from_local_datetime(&day.and_hms_opt(minute / 60, minute % 60, 0)?)
                            .earliest()
                    })
                })
                .map_or(0, |date| date.timestamp_millis().max(0) as u64),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Status {
    pub working: bool,
    pub needs_approval: bool,
    pub done: bool,
}
impl Status {
    pub(crate) fn active(self) -> bool {
        self.working || self.needs_approval || self.done
    }
    pub(crate) fn matches(self, state: Self) -> bool {
        !self.active()
            || (self.working && state.working)
            || (self.needs_approval && state.needs_approval)
            || (self.done && state.done)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Filters {
    pub hidden_providers: Vec<String>,
    pub time: TimeFilter,
    pub status: Status,
    /// Path keys of projects whose sessions are hidden.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hidden_projects: Vec<String>,
    /// Bot involvement: any, sessions a Bot started, ones it did not, or one
    /// Bot's. The caller knows which Bot started a session.
    #[serde(skip_serializing_if = "BotFilter::is_any")]
    pub bot: BotFilter,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BotFilter {
    #[default]
    Any,
    AnyBot,
    NoBot,
    Bot(String),
}
impl BotFilter {
    pub(crate) fn is_any(&self) -> bool {
        *self == Self::Any
    }
    /// Whether a session started by `launcher` (a Bot id, if any) passes.
    pub fn matches(&self, launcher: Option<&str>) -> bool {
        match self {
            Self::Any => true,
            Self::AnyBot => launcher.is_some(),
            Self::NoBot => launcher.is_none(),
            Self::Bot(id) => launcher == Some(id.as_str()),
        }
    }
}
impl Filters {
    pub fn active(&self, show_archived: bool) -> bool {
        show_archived
            || !self.hidden_providers.is_empty()
            || !self.hidden_projects.is_empty()
            || !self.bot.is_any()
            || self.time != TimeFilter::All
            || self.status.active()
    }
    pub fn matches(
        &self,
        session: &Session,
        state: Status,
        start: u64,
        show_archived: bool,
    ) -> bool {
        (!session.archived || show_archived)
            && !self.hidden_providers.contains(&session.provider)
            && !self
                .hidden_projects
                .contains(&crate::files::path_key(&session.cwd))
            && session.updated_at >= start
            && self.status.matches(state)
    }
    pub fn toggle_project(&mut self, key: &str) {
        if self.hidden_projects.iter().any(|k| k == key) {
            self.hidden_projects.retain(|k| k != key);
        } else {
            self.hidden_projects.push(key.into());
        }
    }
    pub fn toggle_provider(&mut self, provider: &str) {
        if Kind::parse(provider).is_none() {
            return;
        }
        if self.hidden_providers.iter().any(|id| id == provider) {
            self.hidden_providers.retain(|id| id != provider);
        } else {
            self.hidden_providers.push(provider.into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bot_and_project_filters_narrow_and_count_as_active() {
        assert!(BotFilter::Any.matches(None));
        assert!(BotFilter::Any.matches(Some("luna")));
        assert!(BotFilter::NoBot.matches(None));
        assert!(!BotFilter::NoBot.matches(Some("luna")));
        assert!(BotFilter::Bot("luna".into()).matches(Some("luna")));
        assert!(!BotFilter::Bot("luna".into()).matches(Some("voky")));
        assert!(!BotFilter::Bot("luna".into()).matches(None));
        let mut filters = Filters::default();
        assert!(!filters.active(false));
        filters.bot = BotFilter::NoBot;
        assert!(filters.active(false));
        filters.bot = BotFilter::Any;
        filters.toggle_project("/work/a");
        assert!(filters.active(false));
        filters.toggle_project("/work/a");
        assert!(filters.hidden_projects.is_empty());
        // Older saved filters without the new fields still load.
        let old: Filters = serde_json::from_str(r#"{"hidden_providers":[],"time":"all"}"#).unwrap();
        assert_eq!(old.bot, BotFilter::Any);
    }

    #[test]
    fn time_boundaries_use_local_midnight_and_rolling_days() {
        let now = Local
            .with_ymd_and_hms(2026, 10, 20, 15, 42, 0)
            .single()
            .unwrap()
            .timestamp_millis() as u64;
        let midnight = Local
            .with_ymd_and_hms(2026, 10, 20, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis() as u64;
        assert_eq!(TimeFilter::Today.start(now), midnight);
        assert_eq!(TimeFilter::Week.start(now), now - 7 * 86_400_000);
        assert_eq!(TimeFilter::Month.start(now), now - 30 * 86_400_000);
        assert_eq!(TimeFilter::Week.start(1), 0);
    }

    #[test]
    fn independent_filters_intersect_and_selected_statuses_are_alternatives() {
        let mut filters = Filters {
            time: TimeFilter::Week,
            status: Status {
                working: true,
                done: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut session = Session {
            id: 1,
            title: "Test".into(),
            provider: "codex".into(),
            cwd: "/project".into(),
            provider_id: None,
            has_messages: true,
            updated_at: 100,
            archived: false,
            pinned: false,
            work_item: None,
        };
        let working = Status {
            working: true,
            ..Default::default()
        };
        assert!(filters.matches(&session, working, 100, false));
        assert!(!filters.matches(&session, working, 101, false));
        assert!(filters.matches(
            &session,
            Status {
                done: true,
                ..Default::default()
            },
            100,
            false
        ));
        assert!(!filters.matches(
            &session,
            Status {
                needs_approval: true,
                ..Default::default()
            },
            100,
            false
        ));
        filters.toggle_provider("codex");
        assert!(!filters.matches(&session, working, 100, true));
        filters.toggle_provider("codex");
        session.archived = true;
        assert!(!filters.matches(&session, working, 100, false));
        assert!(filters.matches(&session, working, 100, true));
        let restored: Filters =
            serde_json::from_str(&serde_json::to_string(&filters).unwrap()).unwrap();
        assert_eq!(restored, filters);
    }
}
