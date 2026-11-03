//! Configuration crosses IPC; audio and recognized text stay in the helper.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DictationConfig {
    pub enabled: bool,
    pub microphone: String,
    pub shortcut: String,
    pub words: String,
}
impl Default for DictationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            microphone: "system".into(),
            shortcut: "fn".into(),
            words: String::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DictationPhase {
    #[default]
    Off,
    Checking,
    Microphone,
    Accessibility,
    Model,
    Downloading,
    Ready,
    Starting,
    Listening,
    Finishing,
    Cancelling,
    Error,
    Unavailable,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DictationStatus {
    pub phase: DictationPhase,
    pub message: String,
    /// Native turn ownership includes asynchronous cleanup, independently of UI.
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WordRule {
    pub heard: String,
    pub preferred: String,
}

/// A preferred spelling per line, optionally preceded by a misheard phrase.
/// The same parser serves Settings and helper admission, including reconnects.
pub fn word_rules(text: &str) -> Result<Vec<WordRule>, String> {
    if text.len() > 16 * 1024 || text.contains('\0') {
        return Err("Custom words must fit within 16 KB and contain no null characters.".into());
    }
    let mut rules = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (heard, preferred) = line
            .split_once('→')
            .or_else(|| line.split_once("=>"))
            .unwrap_or((line, line));
        let (heard, preferred) = (heard.trim(), preferred.trim());
        if heard.is_empty()
            || preferred.is_empty()
            || heard.chars().count() > 120
            || preferred.chars().count() > 120
            || preferred.contains("=>")
            || preferred.contains('→')
        {
            return Err(format!(
                "Line {} needs a word or a heard phrase → preferred spelling (up to 120 characters each).",
                index + 1
            ));
        }
        rules.push(WordRule {
            heard: heard.into(),
            preferred: preferred.into(),
        });
        if rules.len() > 100 {
            return Err("Keep up to 100 custom words or phrases.".into());
        }
    }
    native_word_validation(&rules)?;
    Ok(rules)
}

fn native_word_validation(rules: &[WordRule]) -> Result<(), String> {
    use std::ffi::{CStr, CString, c_char};
    unsafe extern "C" {
        fn vorkspace_dictation_validate_words(
            json: *const c_char,
            reply: extern "C" fn(*const c_char),
        );
    }
    thread_local! { static ERROR: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) }; }
    extern "C" fn reply(error: *const c_char) {
        ERROR.with(|slot| {
            *slot.borrow_mut() = (!error.is_null()).then(|| {
                unsafe { CStr::from_ptr(error) }
                    .to_string_lossy()
                    .into_owned()
            })
        });
    }
    let json = CString::new(serde_json::to_string(rules).unwrap()).unwrap();
    unsafe { vorkspace_dictation_validate_words(json.as_ptr(), reply) };
    ERROR
        .with(|slot| slot.borrow_mut().take())
        .map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_preferences_cannot_enable_capture_and_shortcut_is_not_settings() {
        let config: DictationConfig = serde_json::from_str("{}").unwrap();
        assert!(!config.enabled);
        assert_eq!(config.shortcut, "fn");
        assert_eq!(config.microphone, "system");
    }
    #[test]
    fn words_preserve_spelling_and_validate_the_whole_edit() {
        assert_eq!(
            word_rules(" Vorkspace\nwork space → Vorkspace\nclawed => Claude\n").unwrap(),
            vec![
                WordRule {
                    heard: "Vorkspace".into(),
                    preferred: "Vorkspace".into()
                },
                WordRule {
                    heard: "work space".into(),
                    preferred: "Vorkspace".into()
                },
                WordRule {
                    heard: "clawed".into(),
                    preferred: "Claude".into()
                },
            ]
        );
        assert!(
            word_rules("Good\ninvalid →")
                .unwrap_err()
                .starts_with("Line 2")
        );
        assert!(word_rules(&"term\n".repeat(101)).is_err());
        assert!(word_rules("secret\0term").is_err());
        assert!(word_rules("term → One\nTERM → Other").is_err());
    }
}
