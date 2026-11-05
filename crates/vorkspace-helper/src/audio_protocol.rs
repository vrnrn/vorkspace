//! Text-only recording control. PCM never crosses the helper socket.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use vorkspace_meetings::recording::{CaptureSource, Segment, State};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioRequest {
    pub id: String,
    pub owner: String,
    pub run: String,
    pub previous_run: Option<String>,
    pub source: CaptureSource,
    pub data_dir: PathBuf,
    pub microphone: String,
    pub words: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AudioNotice {
    Idle,
    State {
        owner: String,
        state: State,
    },
    Segment {
        id: String,
        owner: String,
        run: String,
        segment: Segment,
    },
    Partial {
        id: String,
        owner: String,
        run: String,
        source: String,
        text: String,
    },
}
