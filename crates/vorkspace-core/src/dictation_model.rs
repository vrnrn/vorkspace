//! Local dictation artifacts. Identity is persisted; metadata is pinned.
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelId {
    #[default]
    UnifiedEnglish,
    ParakeetTdtV3,
    ParakeetRealtimeEou,
}
pub struct Descriptor {
    pub label: &'static str,
    pub filename: &'static str,
    pub repository: &'static str,
    pub revision: &'static str,
    pub url: &'static str,
    pub bytes: u64,
    /// A single-file artifact's checksum. Multi-file models use a pinned manifest.
    pub sha256: Option<&'static str>,
    pub language: Option<&'static str>,
}
impl ModelId {
    pub const ALL: [Self; 3] = [
        Self::UnifiedEnglish,
        Self::ParakeetTdtV3,
        Self::ParakeetRealtimeEou,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::UnifiedEnglish => "unified_english",
            Self::ParakeetTdtV3 => "parakeet_tdt_v3",
            Self::ParakeetRealtimeEou => "parakeet_realtime_eou",
        }
    }
    pub fn from_key(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.key() == value)
    }
    pub const fn descriptor(self) -> &'static Descriptor {
        match self {
            Self::UnifiedEnglish => &Descriptor {
                label: "Parakeet Unified English",
                filename: "parakeet-unified-en-0.6b-Q8_0.gguf",
                repository: "handy-computer/parakeet-unified-en-0.6b-gguf",
                revision: "7e948f21b7bdbac698d3318db9d350f1096f3b6c",
                url: concat!(
                    "https://huggingface.co/handy-computer/parakeet-unified-en-0.6b-gguf/resolve/",
                    "7e948f21b7bdbac698d3318db9d350f1096f3b6c/parakeet-unified-en-0.6b-Q8_0.gguf"
                ),
                bytes: 731_357_568,
                sha256: Some("4b50b6dd862bf6e346929aaf4f5eaacec003bfa3f56462d6c874b41ef2f38795"),
                language: Some("en"),
            },
            Self::ParakeetTdtV3 => &Descriptor {
                label: "Parakeet-TDT v3",
                filename: "parakeet-tdt-0.6b-v3-Q8_0.gguf",
                repository: "handy-computer/parakeet-tdt-0.6b-v3-gguf",
                revision: "85ac09ea12fc4b1112fa76810059364bc6adc9de",
                url: concat!(
                    "https://huggingface.co/handy-computer/parakeet-tdt-0.6b-v3-gguf/resolve/",
                    "85ac09ea12fc4b1112fa76810059364bc6adc9de/parakeet-tdt-0.6b-v3-Q8_0.gguf"
                ),
                bytes: 739_508_576,
                sha256: Some("5859f77944efcd8eafa23a6350731960b2b55b2203df51f319665c807d802cc7"),
                // The pinned v3 artifact has no language-prompt input.
                language: None,
            },
            Self::ParakeetRealtimeEou => &Descriptor {
                label: "Parakeet Flash",
                filename: "parakeet-realtime-eou-120m-coreml-160ms.eou",
                repository: "FluidInference/parakeet-realtime-eou-120m-coreml",
                revision: "40a23f4c0b333aa17ad8c0f2ea47ec2347f2f355",
                url: concat!(
                    "https://huggingface.co/FluidInference/parakeet-realtime-eou-120m-coreml/resolve/",
                    "40a23f4c0b333aa17ad8c0f2ea47ec2347f2f355/160ms/"
                ),
                bytes: 224_047_838,
                sha256: None,
                language: Some("en"),
            },
        }
    }
}
