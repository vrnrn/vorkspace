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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CompactionMode {
    #[default]
    SessionRollover,
    Native,
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
    /// The body's printed pattern (see `BOT_PATTERNS`); unset reads as dither.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// Where the pattern sits (see `BOT_PRINTS`); unset reads as ground.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub print: Option<String>,
    pub projects: Vec<String>,
    pub show_started_sessions_in_sidebar: bool,
    pub use_sidebar_folders: bool,
    pub compaction_mode: CompactionMode,
    pub background: Option<Background>,
    pub launches: Vec<Launch>,
    pub documents: Vec<Document>,
    /// References last projected from desktop blocks, to reconcile deletions.
    #[serde(default, rename = "desktopDocumentIds")]
    pub projected_document_ids: Vec<String>,
    #[serde(default, rename = "desktopLaunchIds")]
    pub projected_launch_ids: Vec<i64>,
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
            pattern: None,
            print: None,
            projects: Vec::new(),
            show_started_sessions_in_sidebar: false,
            use_sidebar_folders: false,
            compaction_mode: CompactionMode::default(),
            background: None,
            launches: Vec::new(),
            documents: Vec::new(),
            projected_document_ids: Vec::new(),
            projected_launch_ids: Vec::new(),
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
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Launch {
    pub session_id: i64,
    pub cwd: String,
    pub title: String,
    pub harness: String,
    pub model: Option<String>,
    /// Compatibility field for saved launches from older desktop builds.
    pub origin_turn: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<LaunchOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_id: Option<String>,
    pub notify_on_complete: bool,
    #[serde(default)]
    pub completed: bool,
    #[serde(default)]
    pub dismissed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchOrigin {
    pub session_id: i64,
    pub turn: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LaunchGroup {
    Turn(LaunchOrigin),
    LegacyChild(i64),
}

impl LaunchGroup {
    pub fn source(self) -> Option<i64> {
        match self {
            Self::Turn(origin) => Some(origin.session_id),
            Self::LegacyChild(_) => None,
        }
    }
}

impl Launch {
    pub fn set_origin(&mut self, session_id: i64, turn: u64) {
        self.origin = Some(LaunchOrigin { session_id, turn });
        self.origin_turn = ((session_id as u64) << 32) | (turn & u64::from(u32::MAX));
        self.notification_id = self.notify_on_complete.then(crate::session::new_id);
    }
}

impl Bot {
    pub fn launch_group(&self, launch: &Launch) -> LaunchGroup {
        if let Some(origin) = launch.origin {
            return LaunchGroup::Turn(origin);
        }
        let legacy = LaunchGroup::LegacyChild(launch.session_id);
        let prefix = launch.origin_turn >> 32;
        if prefix == 0 {
            return legacy;
        }
        // Older records retained only 32 bits. Recover the source only when
        // one known conversation for this Bot owns that exact prefix.
        let mut source = None;
        for id in self
            .session_id
            .into_iter()
            .chain(self.habit_sessions.iter().copied())
        {
            if id > 0 && u64::from(id as u32) == prefix {
                if source.is_some_and(|previous| previous != id) {
                    return legacy;
                }
                source = Some(id);
            }
        }
        source.map_or(legacy, |session_id| {
            LaunchGroup::Turn(LaunchOrigin {
                session_id,
                turn: launch.origin_turn & u64::from(u32::MAX),
            })
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delegated_origin_retains_a_full_handle_and_a_fresh_notice_for_each_send() {
        let source = (1_i64 << 52) | 13;
        let turn = (1_u64 << 40) | 9;
        let mut launch = Launch {
            session_id: 32,
            notify_on_complete: true,
            ..Default::default()
        };
        launch.set_origin(source, turn);
        let notice = launch.notification_id.clone();
        let encoded = serde_json::to_value(&launch).unwrap();
        let restored: Launch = serde_json::from_value(encoded).unwrap();
        assert_eq!(
            Bot::default().launch_group(&restored),
            LaunchGroup::Turn(LaunchOrigin {
                session_id: source,
                turn
            })
        );
        launch.set_origin(source, turn);
        assert_ne!(launch.notification_id, notice);
    }

    #[test]
    fn legacy_delegation_is_resolved_only_to_a_unique_known_bot_session() {
        let source = (1_i64 << 52) | 13;
        let launch = Launch {
            session_id: 32,
            origin_turn: (13 << 32) | 9,
            notify_on_complete: true,
            ..Default::default()
        };
        let mut owner = Bot {
            session_id: Some(5),
            habit_sessions: vec![source],
            ..Default::default()
        };
        assert_eq!(owner.launch_group(&launch).source(), Some(source));
        owner.habit_sessions.push((1_i64 << 51) | 13);
        assert_eq!(owner.launch_group(&launch), LaunchGroup::LegacyChild(32));
        assert_eq!(
            Bot::default().launch_group(&launch),
            LaunchGroup::LegacyChild(32)
        );
    }
}
