//! Provider-to-view publication of transcript changes and turn outcomes.
use vorkspace_core::{agent::Approval, provider, transcript::Row};

#[derive(Default)]
pub struct Update {
    /// The input was transferred to a durable receipt or journal.
    pub composer_accepted: Option<vorkspace_core::store::ComposerOrigin>,
    /// Full, bounded latest-tool label for this run; None means no known tool.
    pub activity: Option<String>,
    pub transcript: TranscriptPatch,
    pub worktree_cwd: Option<String>,
    pub commands: Option<Vec<provider::NativeCommand>>,
    pub provider_turn_id: Option<String>,
    pub turn_ready: bool,
    pub follow_up_delivered: Vec<String>,
    pub follow_up_errors: Vec<(String, String)>,
    pub delivered: Option<String>,
    pub provider_id: Option<String>,
    pub provider_reset: bool,
    pub model: Option<String>,
    pub config_model: Option<String>,
    pub config_settings: Option<std::collections::HashMap<String, String>>,
    pub context: Option<vorkspace_core::context::Usage>,
    pub metrics: Option<vorkspace_core::agent::TurnMetrics>,
    pub usage_limit: Option<vorkspace_core::usage::Limit>,
    pub approvals: Vec<Approval>,
    pub approvals_resolved: Vec<String>,
    pub questions: Vec<vorkspace_core::agent::Question>,
    pub questions_resolved: Vec<String>,
    pub handoff: Option<Option<vorkspace_core::agent::Handoff>>,
    pub status: String,
    pub outcome: Option<TurnOutcome>,
    /// The losing turn has a separate owner in the same chat. Replace the projected
    /// transcript from storage instead of applying the losing row offsets.
    pub history_recovery: Option<HistoryRecovery>,
}

impl Update {
    pub fn finished(&self) -> bool {
        self.outcome.is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnOutcome {
    Completed,
    Cancelled,
    Superseded,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryRecovery {
    Saved,
    Unsaved,
}

/// Transcript updates use absolute source row indices. Only an authoritative
/// snapshot can remove a previously visible suffix.
#[derive(Default)]
pub enum TranscriptPatch {
    #[default]
    Unchanged,
    Tail {
        start: usize,
        rows: Vec<Row>,
    },
    Snapshot {
        start: usize,
        rows: Vec<Row>,
    },
    /// Includes the complete Bot history admitted for this run, even if the
    /// view skipped the first update while stopping.
    BotSnapshot {
        turn_start: usize,
        rows: Vec<Row>,
    },
}

impl TranscriptPatch {
    pub(super) fn tail(start: usize, rows: Vec<Row>) -> Self {
        if rows.is_empty() {
            Self::Unchanged
        } else {
            Self::Tail { start, rows }
        }
    }

    #[cfg(test)]
    pub(super) fn rows(&self) -> &[Row] {
        match self {
            Self::Unchanged => &[],
            Self::Tail { rows, .. }
            | Self::Snapshot { rows, .. }
            | Self::BotSnapshot { rows, .. } => rows,
        }
    }

    #[cfg(test)]
    pub(super) fn into_rows(self) -> Vec<Row> {
        match self {
            Self::Unchanged => Vec::new(),
            Self::Tail { rows, .. }
            | Self::Snapshot { rows, .. }
            | Self::BotSnapshot { rows, .. } => rows,
        }
    }
}
