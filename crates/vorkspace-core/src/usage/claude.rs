use super::{now_ms, parse_claude, FetchError, Snapshot};
use serde_json::Value;
use std::{io::Read, path::PathBuf, time::Duration};

pub(super) fn fetch_for(profile: &crate::accounts::Profile) -> Result<Snapshot, FetchError> {
    if let Some(snapshot) =
        crate::catalog::cli_json_for(crate::provider::Kind::Claude, &["auth", "status"], profile)
            .ok()
            .and_then(|v| backend_usage(&v))
    {
        return Ok(snapshot);
    }
    let config = profile
        .directory(crate::provider::Kind::Claude)
        .map_err(FetchError::failed)?
        .or_else(config_dir);
    let credentials = credentials(config).ok_or_else(|| {
        FetchError::unavailable("Sign in with claude auth login, then refresh usage.")
    })?;
    let oauth = credentials.get("claudeAiOauth").unwrap_or(&credentials);
    let token = oauth["accessToken"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| FetchError::unavailable("Claude is not signed in."))?;
    let expiry = oauth["expiresAt"]
        .as_i64()
        .or_else(|| oauth["expiresAt"].as_str()?.parse::<i64>().ok());
    // The CLI owns its rotating refresh token. Never refresh or write it here.
    if expiry.is_some_and(|at| at <= now_ms()) {
        return Err(FetchError::unavailable(
            "Claude sign-in expired. Run claude auth login, then refresh.",
        ));
    }
    let response = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .build()
        .get("https://api.anthropic.com/api/oauth/usage")
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", "oauth-2025-04-20")
        .set("User-Agent", "claude-code/2.1.0")
        .call()
        .map_err(|error| match error {
            ureq::Error::Status(401, _) => FetchError::unavailable(
                "Claude sign-in expired. Run claude auth login, then refresh.",
            ),
            ureq::Error::Status(403, _) => {
                FetchError::unavailable("Claude usage is unavailable for this account.")
            }
            ureq::Error::Status(status, _) => {
                FetchError::failed(format!("Claude usage request failed ({status})."))
            }
            _ => FetchError::failed("Could not reach Claude usage. Try refreshing again."),
        })?;
    let mut body = String::new();
    response
        .into_reader()
        .take(1024 * 1024)
        .read_to_string(&mut body)
        .map_err(|_| FetchError::failed("Could not read Claude usage response."))?;
    let value = serde_json::from_str(&body)
        .map_err(|_| FetchError::failed("Claude usage response was not JSON."))?;
    parse_claude(&value)
}

fn config_dir() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}
fn credentials(config: Option<PathBuf>) -> Option<Value> {
    if let Some(value) = keychain(config.as_deref()) {
        return Some(value);
    }
    let dir = config.or_else(|| {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(|home| PathBuf::from(home).join(".claude"))
    })?;
    let raw = std::fs::read_to_string(dir.join(".credentials.json")).ok()?;
    credential_blob(&raw)
}
fn credential_blob(raw: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(raw.trim()).ok()?;
    value.get("claudeAiOauth").unwrap_or(&value)["accessToken"]
        .as_str()
        .filter(|s| !s.trim().is_empty())?;
    Some(value)
}

pub(crate) fn delete_keychain(config: &std::path::Path) -> Result<(), String> {
    use crate::process;
    use std::{
        process::{Command, Stdio},
        time::Instant,
    };
    let service = keychain_service(Some(config));
    let mut command = Command::new("/usr/bin/security");
    command
        .args(["delete-generic-password", "-s", &service])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    process::isolate(&mut command);
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let _registration = process::register(child.id());
    let deadline = Instant::now() + Duration::from_secs(5);
    let result = loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() || status.code() == Some(44) => break Ok(()),
            Ok(Some(_)) => {
                break Err("Could not remove the account's Claude Keychain credentials".into());
            }
            Err(e) => break Err(e.to_string()),
            Ok(None) if Instant::now() >= deadline => {
                break Err("Removing Claude credentials timed out".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    process::terminate(&mut child);
    result
}

fn keychain_service(config: Option<&std::path::Path>) -> String {
    use sha2::{Digest, Sha256};
    use unicode_normalization::UnicodeNormalization;
    config
        .map(|dir| {
            let selector: String = dir.to_string_lossy().nfc().collect();
            let digest = format!("{:x}", Sha256::digest(selector.as_bytes()));
            format!("Claude Code-credentials-{}", &digest[..8])
        })
        .unwrap_or_else(|| "Claude Code-credentials".into())
}

fn keychain(config: Option<&std::path::Path>) -> Option<Value> {
    use crate::process;
    use std::{
        process::{Command, Stdio},
        sync::mpsc,
        thread,
        time::Instant,
    };

    let service = keychain_service(config);
    let mut command = Command::new("/usr/bin/security");
    command
        .args(["find-generic-password", "-s", &service, "-w"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    process::isolate(&mut command);
    let mut child = command.spawn().ok()?;
    let _registration = process::register(child.id());
    let output = child.stdout.take()?;
    // Drain concurrently: waiting before reading can fill the pipe and hang.
    let (tx, rx) = mpsc::sync_channel(1);
    let reader = thread::spawn(move || {
        let mut body = String::new();
        let result = output.take(1024 * 1024).read_to_string(&mut body);
        let _ = tx.send(result.ok().map(|_| body));
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let result = loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                break rx
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .ok()
                    .flatten();
            }
            Ok(Some(_)) | Err(_) => break None,
            Ok(None) if Instant::now() >= deadline => break None,
            Ok(None) => thread::sleep(Duration::from_millis(20)),
        }
    };
    process::terminate(&mut child);
    drop(rx);
    let _ = reader.join();
    credential_blob(&result?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_nested_and_legacy_credentials_without_logging_secrets() {
        assert!(
            credential_blob(r#"{"claudeAiOauth":{"accessToken":"test","expiresAt":123}}"#)
                .is_some()
        );
        assert!(credential_blob(r#"{"accessToken":"test"}"#).is_some());
        assert!(credential_blob(r#"{"accessToken":" "}"#).is_none());
        assert!(credential_blob("invalid").is_none());
    }
}

fn backend_usage(value: &Value) -> Option<Snapshot> {
    (value["apiProvider"]
        .as_str()?
        .eq_ignore_ascii_case("bedrock")
        && value["loggedIn"] != false)
        .then(|| external_usage("AWS Bedrock"))
}
pub(super) fn external_usage(name: &str) -> Snapshot {
    Snapshot {
        updated_at: now_ms(),
        external_billing: Some(name.into()),
        ..Default::default()
    }
}
#[cfg(test)]
mod backend_tests {
    use super::*;
    #[test]
    fn bedrock_probe_uses_cli_status_and_the_selected_profile_without_oauth() {
        use std::{os::unix::fs::PermissionsExt, process::Command};
        const CHILD: &str = "VORKSPACE_BEDROCK_PROBE_TEST_ROOT";
        if let Some(root) = std::env::var_os(CHILD) {
            let profile = crate::accounts::Profile::new("bedrock-test", &PathBuf::from(root));
            let snapshot = fetch_for(&profile).unwrap();
            assert_eq!(snapshot.external_billing.as_deref(), Some("AWS Bedrock"));
            assert!(snapshot.windows.is_empty());
            return;
        }
        // Process-local overrides keep the real CLI, credentials, and parallel tests untouched.
        let root = tempfile::tempdir().unwrap();
        let binary = root.path().join("claude");
        std::fs::write(
            &binary,
            r#"#!/bin/sh
test "$#" = 2 && test "$1" = auth && test "$2" = status || exit 1
test "$CLAUDE_CODE_USE_BEDROCK" = 1 && test "$AWS_REGION" = us-east-1 || exit 2
test "$CLAUDE_CONFIG_DIR" = "$VORKSPACE_BEDROCK_PROBE_TEST_ROOT/provider-accounts/claude/bedrock-test" || exit 3
printf '%s\n' '{"apiProvider":"bedrock","loggedIn":true,"authMethod":"third_party"}'
"#,
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "usage::claude::backend_tests::bedrock_probe_uses_cli_status_and_the_selected_profile_without_oauth",
                "--nocapture",
            ])
            .env(CHILD, root.path())
            .env("VORKSPACE_CLAUDE_BINARY", binary)
            .env("CLAUDE_CODE_USE_BEDROCK", "1")
            .env("AWS_REGION", "us-east-1")
            .env("SHELL", "/bin/false")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    #[test]
    fn bedrock_is_connected_without_an_oauth_subscription() {
        let snapshot =
            backend_usage(&serde_json::json!({"apiProvider":"bedrock","loggedIn":true})).unwrap();
        assert!(snapshot.windows.is_empty());
        assert_eq!(snapshot.external_billing.as_deref(), Some("AWS Bedrock"));
        assert!(
            backend_usage(&serde_json::json!({"apiProvider":"bedrock","loggedIn":false})).is_none()
        );
        assert!(backend_usage(&serde_json::json!({"apiProvider":"firstParty"})).is_none());
    }
}
