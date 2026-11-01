//! A provider usage limit, as recorded on a session.
use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Limit {
    pub resets_at: Option<i64>,
    pub resume_at_reset: bool,
}
impl Limit {
    pub fn from_error(message: &str) -> Option<Self> {
        // Claude's synthetic assistant errors use this code before the result
        // frame arrives. Ordinary transient HTTP 429s must remain retryable.
        if message.trim().eq_ignore_ascii_case("rate_limit") {
            return Some(Self::default());
        }
        static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let pattern=PATTERN.get_or_init(||regex::Regex::new(r"(?i)(?:usage|spending|monthly|weekly|daily)[ _]limit[ _](?:reached|exceeded)|(?:quota|credits?) (?:exceeded|exhausted|depleted)|insufficient[_ ](?:quota|credits)|hit your (?:(?:usage|session) )?limit|credit balance is too low|reached (?:its|your) usage limit").unwrap());
        pattern.is_match(message).then(Self::default)
    }
    /// Extra-usage refusals fall back to the subscription and do not stop a turn.
    pub fn from_claude_info(info: &Value) -> Option<Self> {
        (info["status"] == "rejected"
            && info["isUsingOverage"] != true
            && info["rateLimitType"] != "overage")
            .then(|| Self {
                resets_at: timestamp(&info["resetsAt"]),
                resume_at_reset: false,
            })
    }
    pub fn resume_due(&self, now: i64, busy: bool) -> bool {
        !busy
            && self.resume_at_reset
            && self
                .resets_at
                .is_some_and(|at| now >= at.saturating_add(30_000))
    }
}

pub(crate) fn timestamp(value: &Value) -> Option<i64> {
    let numeric = value
        .as_f64()
        .or_else(|| value.as_str()?.parse::<f64>().ok());
    if let Some(n) = numeric.filter(|n| n.is_finite() && *n > 0.) {
        return Some(if n > 10_000_000_000. { n } else { n * 1000. } as i64);
    }
    chrono::DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|d| d.timestamp_millis())
}
