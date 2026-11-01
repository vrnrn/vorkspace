use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PiUsageProvider {
    Anthropic,
    OpenaiCodex,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageWindow {
    pub used_percent: f64,
    pub window_minutes: u64,
    pub resets_at: Option<u64>,
}
#[derive(Debug, Default, Serialize)]
pub struct UsageWindows {
    pub session: Option<UsageWindow>,
    pub weekly: Option<UsageWindow>,
}
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum PiUsageResult {
    Ok { windows: UsageWindows },
    Unavailable { message: String },
    Error { message: String },
}

#[derive(PartialEq)]
struct PiOAuth {
    access: String,
    expires: u64,
    account_id: Option<String>,
}

impl PiUsageProvider {
    fn key(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenaiCodex => "openai-codex",
        }
    }
    fn endpoint(self) -> &'static str {
        match self {
            Self::Anthropic => "https://api.anthropic.com/api/oauth/usage",
            Self::OpenaiCodex => "https://chatgpt.com/backend-api/wham/usage",
        }
    }
}

pub(super) fn fetch_pi_usage(provider: PiUsageProvider) -> PiUsageResult {
    let Some(dir) = agent_dir(
        std::env::var_os("PI_CODING_AGENT_DIR"),
        crate::shell_env::home().map(|p| p.to_string_lossy().into_owned()),
    ) else {
        return unavailable(
            "Pi usage needs an absolute configuration directory. Check PI_CODING_AGENT_DIR.",
        );
    };
    read_usage(&dir, provider, |credential| {
        request_usage(provider, credential, provider.endpoint())
    })
}

fn agent_dir(override_dir: Option<std::ffi::OsString>, home: Option<String>) -> Option<PathBuf> {
    if let Some(value) = override_dir.filter(|v| !v.is_empty()) {
        let path = PathBuf::from(value);
        if let Ok(suffix) = path.strip_prefix("~") {
            return home.map(|h| PathBuf::from(h).join(suffix));
        }
        return path.is_absolute().then_some(path);
    }
    home.map(|h| PathBuf::from(h).join(".pi/agent"))
}

fn unavailable(message: &str) -> PiUsageResult {
    PiUsageResult::Unavailable {
        message: message.into(),
    }
}
fn error(message: &str) -> PiUsageResult {
    PiUsageResult::Error {
        message: message.into(),
    }
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn bounded_text(reader: impl Read) -> Result<String, ()> {
    let mut text = String::new();
    reader
        .take(MAX_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|_| ())?;
    if text.len() as u64 > MAX_BYTES {
        return Err(());
    }
    Ok(text)
}
fn read_json(path: &Path) -> Result<Option<Value>, PiUsageResult> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(error(
                "Could not read Pi configuration. Check its file permissions.",
            ));
        }
    };
    let text =
        bounded_text(file).map_err(|_| error("Pi configuration is too large or unreadable."))?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|_| error("Pi configuration contains invalid JSON."))
}
fn has_auth_override(value: &Value) -> bool {
    value.as_object().is_some_and(|obj| {
        ["apiKey", "baseUrl", "headers", "authHeader"]
            .iter()
            .any(|key| obj.contains_key(*key))
            || obj
                .get("models")
                .and_then(Value::as_array)
                .is_some_and(|models| models.iter().any(has_auth_override))
            || obj
                .get("modelOverrides")
                .and_then(Value::as_object)
                .is_some_and(|models| models.values().any(has_auth_override))
    })
}
fn credential(dir: &Path, provider: PiUsageProvider) -> Result<PiOAuth, PiUsageResult> {
    if let Some(config) = read_json(&dir.join("models.json"))? {
        if has_auth_override(&config["providers"][provider.key()]) {
            return Err(unavailable(
                "Usage is unavailable for custom Pi authentication or endpoints.",
            ));
        }
    }
    let value = read_json(&dir.join("auth.json"))?.unwrap_or(Value::Null);
    let entry = &value[provider.key()];
    if entry["type"].as_str() == Some("api_key") {
        return Err(unavailable(
            "Subscription usage is unavailable for Pi API keys.",
        ));
    }
    if entry["type"].as_str() != Some("oauth") {
        return Err(unavailable(
            "Sign in to this provider through Pi, then refresh usage.",
        ));
    }
    let access = entry["access"].as_str().filter(|v| valid_header(v));
    let expires = entry["expires"].as_u64();
    let (Some(access), Some(expires)) = (access, expires) else {
        return Err(error(
            "Pi credentials are invalid. Sign in through Pi again.",
        ));
    };
    if expires <= now_ms() {
        return Err(unavailable(
            "Pi sign-in expired. Resume or sign in through Pi, then refresh usage.",
        ));
    }
    let account_id = entry["accountId"]
        .as_str()
        .filter(|v| valid_header(v))
        .map(str::to_owned);
    if matches!(provider, PiUsageProvider::OpenaiCodex) && account_id.is_none() {
        return Err(unavailable(
            "Pi's Codex account could not be identified. Sign in through Pi again.",
        ));
    }
    Ok(PiOAuth {
        access: access.into(),
        expires,
        account_id,
    })
}
fn valid_header(value: &str) -> bool {
    !value.trim().is_empty() && value.bytes().all(|c| (0x21..=0x7e).contains(&c))
}
fn read_usage(
    dir: &Path,
    provider: PiUsageProvider,
    request: impl FnOnce(&PiOAuth) -> PiUsageResult,
) -> PiUsageResult {
    let credentials = match credential(dir, provider) {
        Ok(value) => value,
        Err(result) => return result,
    };
    let result = request(&credentials);
    // Pi owns token rotation. Never refresh or write its credentials here.
    if credential(dir, provider).ok().as_ref() != Some(&credentials) {
        return error("Pi credentials changed. Refresh usage to read the current account.");
    }
    result
}
fn request_usage(provider: PiUsageProvider, credentials: &PiOAuth, url: &str) -> PiUsageResult {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .redirects(0)
        .build();
    let mut request = agent
        .get(url)
        .set("Authorization", &format!("Bearer {}", credentials.access))
        .set("User-Agent", "vorkspace");
    match provider {
        PiUsageProvider::Anthropic => request = request.set("anthropic-beta", "oauth-2025-04-20"),
        PiUsageProvider::OpenaiCodex => {
            if let Some(id) = &credentials.account_id {
                request = request.set("ChatGPT-Account-Id", id);
            }
        }
    }
    let response = match request.call() {
        Ok(response) if response.status() == 200 => response,
        Err(ureq::Error::Status(401, _)) => {
            return unavailable("Pi sign-in expired. Sign in through Pi, then refresh usage.");
        }
        Err(ureq::Error::Status(403, _)) => {
            return unavailable("Usage is unavailable for this Pi account.");
        }
        _ => return error("Could not fetch Pi usage. Try refreshing."),
    };
    match bounded_text(response.into_reader()) {
        Ok(body) => parse_usage(provider, &body),
        Err(_) => error("Pi usage response was too large or unreadable."),
    }
}
fn parse_usage(provider: PiUsageProvider, body: &str) -> PiUsageResult {
    let parsed = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| parse_windows(provider, &value));
    match parsed {
        Some(windows) if windows.session.is_some() || windows.weekly.is_some() => {
            PiUsageResult::Ok { windows }
        }
        _ => error("Pi usage response was unexpected. Try refreshing."),
    }
}
fn parse_windows(provider: PiUsageProvider, value: &Value) -> Option<UsageWindows> {
    let mut windows = UsageWindows::default();
    match provider {
        PiUsageProvider::Anthropic => {
            for (key, minutes) in [("five_hour", 300), ("seven_day", 10080)] {
                let raw = &value[key];
                if raw.is_null() {
                    continue;
                }
                let window = UsageWindow {
                    used_percent: percentage(&raw["utilization"])?,
                    window_minutes: minutes,
                    resets_at: if raw["resets_at"].is_null() {
                        None
                    } else {
                        let date = chrono::DateTime::parse_from_rfc3339(raw["resets_at"].as_str()?)
                            .ok()?;
                        Some(u64::try_from(date.timestamp_millis()).ok()?)
                    },
                };
                if minutes == 300 {
                    windows.session = Some(window);
                } else {
                    windows.weekly = Some(window);
                }
            }
        }
        PiUsageProvider::OpenaiCodex => {
            let limits = value.get("rate_limit")?.as_object()?;
            for key in ["primary_window", "secondary_window"] {
                let Some(raw) = limits.get(key).filter(|v| !v.is_null()) else {
                    continue;
                };
                let seconds = raw["limit_window_seconds"].as_u64()?;
                if seconds == 0 || seconds % 60 != 0 {
                    return None;
                }
                let window = UsageWindow {
                    used_percent: percentage(&raw["used_percent"])?,
                    window_minutes: seconds / 60,
                    resets_at: if raw["reset_at"].is_null() {
                        None
                    } else {
                        Some(raw["reset_at"].as_u64()?.checked_mul(1000)?)
                    },
                };
                let slot = if seconds >= 604800 {
                    &mut windows.weekly
                } else {
                    &mut windows.session
                };
                if slot.is_some() {
                    return None;
                }
                *slot = Some(window);
            }
        }
    }
    Some(windows)
}
fn percentage(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .filter(|v| v.is_finite() && (0.0..=100.0).contains(v))
}

#[cfg(test)]
mod tests;
