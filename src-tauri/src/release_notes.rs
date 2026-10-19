use std::io::Read;
use std::time::Duration;

use serde::Deserialize;

const MAX_RESPONSE_BYTES: u64 = 256 * 1024;

fn release_url(version: &str) -> Result<String, String> {
    let parts: Vec<_> = version.split('.').collect();
    if version.len() > 64
        || parts.len() != 3
        || parts.iter().any(|part| {
            part.is_empty()
                || !part.bytes().all(|b| b.is_ascii_digit())
                || (part.len() > 1 && part.starts_with('0'))
        })
    {
        return Err("Invalid release version".into());
    }
    Ok(format!(
        "https://api.github.com/repos/vrnrn/vorkspace/releases/tags/mac-v{version}"
    ))
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    body: Option<String>,
}

fn release_markdown(version: &str, reader: impl Read) -> Result<Option<String>, String> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not read release notes".to_string())?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err("Release notes response is too large".into());
    }
    let release: Release =
        serde_json::from_slice(&bytes).map_err(|_| "Invalid release notes response".to_string())?;
    if release.tag_name != format!("mac-v{version}") || release.draft || release.prerelease {
        return Err("Release notes do not match the requested published version".into());
    }
    Ok(release
        .body
        .filter(|body| !body.trim().is_empty())
        .map(|body| format!("## [{version}]\n\n{}", body.trim())))
}

/// Use one fixed public endpoint, without credentials or a broader webview CSP.
#[tauri::command]
pub async fn read_release_notes(version: String) -> Result<Option<String>, String> {
    let url = release_url(&version)?;
    tauri::async_runtime::spawn_blocking(move || {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(10))
            .redirects(0)
            .build();
        let response = match agent
            .get(&url)
            .set("User-Agent", "vorkspace")
            .set("Accept", "application/vnd.github+json")
            .call()
        {
            Ok(response) if response.status() == 200 => response,
            Err(ureq::Error::Status(404, _)) => return Ok(None),
            _ => return Err("Could not load published release notes".into()),
        };
        release_markdown(&version, response.into_reader())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_only_stable_versions_in_the_fixed_repository() {
        assert_eq!(
            release_url("0.13.10").unwrap(),
            "https://api.github.com/repos/vrnrn/vorkspace/releases/tags/mac-v0.13.10"
        );
        for version in [
            "",
            "../latest",
            "1.2",
            "1.2.3/other",
            "01.2.3",
            "1.2.3?x",
            "1.2.٣",
        ] {
            assert!(release_url(version).is_err(), "{version}");
        }
        assert!(release_url(&format!("1.2.{}", "9".repeat(65))).is_err());
    }

    #[test]
    fn preserves_the_complete_matching_published_body() {
        let body = json!({"tag_name":"mac-v0.13.10", "draft":false, "prerelease":false,
            "body":"## What's Changed\n\n- Bot migration fixes."})
        .to_string();
        assert_eq!(
            release_markdown("0.13.10", body.as_bytes())
                .unwrap()
                .unwrap(),
            "## [0.13.10]\n\n## What's Changed\n\n- Bot migration fixes."
        );
        assert!(release_markdown("0.13.9", body.as_bytes()).is_err());
        for key in ["draft", "prerelease"] {
            let mut value: serde_json::Value = serde_json::from_str(&body).unwrap();
            value[key] = true.into();
            assert!(release_markdown("0.13.10", value.to_string().as_bytes()).is_err());
        }
    }

    #[test]
    fn distinguishes_empty_missing_malformed_and_oversized_bodies() {
        for body in [serde_json::Value::Null, json!(" \n")] {
            let value =
                json!({"tag_name":"mac-v0.13.10", "draft":false, "prerelease":false, "body":body});
            assert_eq!(
                release_markdown("0.13.10", value.to_string().as_bytes()).unwrap(),
                None
            );
        }
        assert!(release_markdown("0.13.10", &b"{}"[..]).is_err());
        assert!(
            release_markdown("0.13.10", &vec![b' '; MAX_RESPONSE_BYTES as usize + 1][..])
                .unwrap_err()
                .contains("too large")
        );
    }
}
