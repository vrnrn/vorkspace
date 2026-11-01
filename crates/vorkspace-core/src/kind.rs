//! Provider identity is shared by discovery, persistence, pickers and transports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Codex,
    Claude,
    Antigravity,
    Pi,
    Omp,
    /// Historical or unrecognized IDs stay readable but cannot start a CLI.
    Unsupported,
}
impl Kind {
    pub const ALL: [Self; 5] = [
        Self::Claude,
        Self::Codex,
        Self::Pi,
        Self::Omp,
        Self::Antigravity,
    ];
    pub fn id(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Antigravity => "antigravity",
            Self::Pi => "pi",
            Self::Omp => "omp",
            Self::Unsupported => "unsupported",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
            Self::Antigravity => "Antigravity",
            Self::Pi => "Pi",
            Self::Omp => "omp",
            Self::Unsupported => "Unavailable provider",
        }
    }
    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.id() == id)
    }
    pub fn from_id(id: &str) -> Self {
        Self::parse(id).unwrap_or(Self::Unsupported)
    }
    /// Presentation of persisted identities is independent of CLI admission.
    pub fn display_label(id: &str) -> &'static str {
        match id {
            "cursor" => "Cursor",
            "grok" => "Grok Build",
            "opencode" => "OpenCode",
            "fx" => "fx",
            "hermes" => "Hermes Agent",
            "devin" => "Devin",
            _ => Self::from_id(id).label(),
        }
    }
    pub fn is_supported(self) -> bool {
        self != Self::Unsupported
    }
    pub fn binary_name(self) -> &'static str {
        match self {
            Self::Antigravity => "agy_acp_server.par",
            _ => self.id(),
        }
    }
    pub(super) fn install_dirs(self) -> &'static [&'static str] {
        match self {
            Self::Antigravity => &[".local/share/agy-acp"],
            _ => &[],
        }
    }
    pub fn is_acp(self) -> bool {
        self == Self::Antigravity
    }
    pub fn is_pi(self) -> bool {
        matches!(self, Self::Pi | Self::Omp)
    }
    pub fn subscription_usage(self) -> bool {
        matches!(self, Self::Codex | Self::Claude)
    }
    pub fn login_hint(self) -> &'static str {
        match self {
            Self::Codex => "codex login",
            Self::Claude => "claude auth login",
            Self::Antigravity => "agy",
            Self::Pi => "pi",
            Self::Omp => "omp",
            Self::Unsupported => "a supported provider",
        }
    }
}

pub fn supports_rewind(kind: Kind) -> bool {
    matches!(kind, Kind::Codex | Kind::Pi | Kind::Omp)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registry_contains_only_supported_providers_and_never_rebinds_history() {
        assert_eq!(
            Kind::ALL.map(Kind::id),
            ["claude", "codex", "pi", "omp", "antigravity"]
        );
        for kind in Kind::ALL {
            assert_eq!(Kind::parse(kind.id()), Some(kind));
        }
        for id in [
            "cursor", "grok", "opencode", "fx", "hermes", "devin", "unknown",
        ] {
            assert_eq!(Kind::parse(id), None);
            assert_eq!(Kind::from_id(id), Kind::Unsupported);
            assert!(!Kind::from_id(id).is_supported());
            assert!(!Kind::from_id(id).is_acp());
        }
    }
    #[test]
    fn historical_labels_do_not_admit_retired_executables() {
        for (id, label) in [
            ("cursor", "Cursor"),
            ("grok", "Grok Build"),
            ("opencode", "OpenCode"),
            ("fx", "fx"),
            ("hermes", "Hermes Agent"),
            ("devin", "Devin"),
        ] {
            assert_eq!(Kind::display_label(id), label);
            assert_eq!(Kind::parse(id), None);
            assert_eq!(Kind::from_id(id), Kind::Unsupported);
            assert_eq!(
                crate::session::display_title(&format!("New {label} session"), id),
                "New session"
            );
            assert_eq!(
                crate::session::display_title("My saved title", id),
                "My saved title"
            );
        }
        for kind in Kind::ALL {
            assert_eq!(Kind::display_label(kind.id()), kind.label());
        }
        assert_eq!(Kind::display_label("unknown"), "Unavailable provider");
    }
}
