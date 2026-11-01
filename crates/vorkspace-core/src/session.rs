//! Identity shared by every session-owning module, independent of how it is stored.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Session {
    pub id: i64,
    pub title: String,
    pub provider: String,
    pub cwd: String,
    pub provider_id: Option<String>,
    pub has_messages: bool,
    /// Unix milliseconds; zero means an older session has no recorded time.
    pub updated_at: u64,
    pub archived: bool,
    pub pinned: bool,
    pub work_item: Option<crate::session_groups::WorkItem>,
}

pub fn native_model(model: &str, provider: &str) -> Option<String> {
    let model = model.trim();
    let model = model.strip_prefix(&format!("{provider}:")).unwrap_or(model);
    let model = model.split('[').next().unwrap_or(model);
    if model.is_empty() || model == "default" {
        return None;
    }
    let model = if provider == "claude"
        && !model.starts_with("claude-")
        && model.chars().any(|c| c.is_ascii_digit())
    {
        format!("claude-{}", model.replace('.', "-"))
    } else {
        model.into()
    };
    Some(model)
}

pub fn new_id() -> String {
    format!("{:032x}", rand::random::<u128>())
}

pub(crate) fn desktop_session_handle(id: &str) -> i64 {
    let bytes = <sha2::Sha256 as sha2::Digest>::digest(id.as_bytes());
    (i64::from_le_bytes(bytes[..8].try_into().unwrap()) & ((1_i64 << 53) - 1)).max(1)
}

/// Tauri's sessionDisplayTitle; persistence retains its canonical provider prefix.
pub fn display_title<'a>(title: &'a str, provider: &str) -> &'a str {
    let prefix = format!("{provider} · ");
    if let Some(title) = title.strip_prefix(&prefix) {
        return title;
    }
    if title == provider
        || title == crate::kind::Kind::display_label(provider)
        || title == format!("New {} session", crate::kind::Kind::display_label(provider))
    {
        return "New session";
    }
    title
}
