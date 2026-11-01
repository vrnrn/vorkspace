//! The Bot records, without behavior. `agent::Settings` and these records refer
//! to each other, so they sit together below everything that acts on them.
use crate::project_appearance::{Background, COLORS, MASCOTS};
use serde::{Deserialize, Serialize};

pub const COLORS_BOT: [u32; 9] = [
    COLORS[1], COLORS[2], COLORS[3], COLORS[4], COLORS[5], COLORS[6], COLORS[7], COLORS[8],
    0x8b63e9,
];
pub const DEFAULT_SOUL: &str =
    "# Soul\n\n## Standing instructions\n- (Add anything it should always keep in mind.)\n";
pub use crate::session::new_id;
fn default_bot_shape() -> String {
    "squircle".into()
}
fn default_bot_textured() -> bool {
    true
}
fn is_true(value: &bool) -> bool {
    *value
}
fn is_default_shape(shape: &str) -> bool {
    shape.is_empty() || shape == "squircle"
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Bot {
    pub id: String,
    pub session_id: Option<i64>,
    /// Keep a missing desktop association visible instead of silently starting
    /// a replacement conversation when its saved UUID cannot be resolved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_session: Option<String>,
    pub name: String,
    /// Committed setup choices remain durable until the backing session owns
    /// them. Publishing its session ID clears this bootstrap configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_configuration: Option<Box<InitialConfiguration>>,
    pub mascot: String,
    #[serde(
        default = "default_bot_shape",
        skip_serializing_if = "is_default_shape"
    )]
    pub shape: String,
    pub color: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fg_color: Option<u32>,
    #[serde(default = "default_bot_textured", skip_serializing_if = "is_true")]
    pub textured: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid: Option<u8>,
    pub projects: Vec<String>,
    pub show_started_sessions_in_sidebar: bool,
    pub use_sidebar_folders: bool,
    pub background: Option<Background>,
    pub launches: Vec<Launch>,
    pub documents: Vec<Document>,
    /// References last projected from desktop blocks, to reconcile deletions.
    #[serde(default)]
    pub desktop_document_ids: Vec<String>,
    #[serde(default)]
    pub desktop_launch_ids: Vec<i64>,
    pub outbox: Vec<Message>,
    pub pending_reports: Vec<String>,
    pub habit_sessions: Vec<i64>,
    pub seen_rows: usize,
    /// Persistent team membership. The actor owns its history and outbox;
    /// its parent owns shared soul, memory, todos and project assignments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minion: Option<MinionMetadata>,
    /// Experimental helpers it spawned, sharing its memory.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub minions: Vec<Minion>,
}
/// A project session a Bot spawned to work alongside it. It acts with its
/// parent's app capability, so it reads and writes the parent's memory, which
/// it shares with the parent and every other minion.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Minion {
    pub name: String,
    pub session_id: i64,
    pub project: String,
    pub created_at: u64,
}
impl Default for Bot {
    fn default() -> Self {
        Self {
            id: new_id(),
            session_id: None,
            unavailable_session: None,
            name: String::new(),
            initial_configuration: None,
            mascot: MASCOTS[0].into(),
            shape: "squircle".into(),
            color: COLORS_BOT[0],
            fg_color: None,
            textured: true,
            seed: None,
            grid: None,
            projects: Vec::new(),
            show_started_sessions_in_sidebar: false,
            use_sidebar_folders: false,
            background: None,
            launches: Vec::new(),
            documents: Vec::new(),
            desktop_document_ids: Vec::new(),
            desktop_launch_ids: Vec::new(),
            outbox: Vec::new(),
            pending_reports: Vec::new(),
            habit_sessions: Vec::new(),
            seen_rows: 0,
            minions: Vec::new(),
            minion: None,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InitialConfiguration {
    pub provider: String,
    pub settings: crate::agent::Settings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soul: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Launch {
    pub session_id: i64,
    pub cwd: String,
    pub title: String,
    pub harness: String,
    pub model: Option<String>,
    pub origin_turn: u64,
    pub notify_on_complete: bool,
    #[serde(default)]
    pub completed: bool,
    #[serde(default)]
    pub dismissed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Document {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub summary: String,
    pub body: String,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub text: String,
    /// Saved transport context, kept separate from the visible message text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepared_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer: Option<Peer>,
    #[serde(default)]
    pub sent_at: Option<u64>,
    #[serde(default)]
    pub internal: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub attachments: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MinionMetadata {
    pub parent_id: String,
    pub role: String,
    pub specialties: Vec<String>,
    pub retired: bool,
    pub created_at: u64,
    /// Refreshed from the roster, never a second persisted authority.
    #[serde(skip)]
    pub parent_name: String,
    #[serde(skip)]
    pub team: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    pub from: String,
    pub hop: u8,
    pub expect_reply: bool,
}
