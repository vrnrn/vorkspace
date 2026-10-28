//! Launch-only harness update detection. All I/O runs on a worker; installation
//! occurs only when the user chooses Update. Nothing is persisted.
use crate::{
    catalog, cli,
    provider::{self, Kind},
};
use std::{
    cmp::Ordering,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering as AtomicOrdering},
    time::Duration,
};

static CLAIMED: AtomicBool = AtomicBool::new(false);
pub fn claim_launch_check() -> bool {
    !CLAIMED.swap(true, AtomicOrdering::SeqCst)
}
#[derive(Clone, Debug)]
pub struct Update {
    pub harness: Kind,
    pub installed: String,
    pub latest: String,
}
fn package(kind: Kind) -> Option<&'static str> {
    match kind {
        Kind::Claude => Some("@anthropic-ai/claude-code"),
        Kind::Codex => Some("@openai/codex"),
        Kind::Pi => Some("@earendil-works/pi-coding-agent"),
        _ => None,
    }
}
pub fn parse_version(text: &str) -> Option<String> {
    regex::Regex::new(r"\d+\.\d+\.\d+")
        .ok()?
        .find(text)
        .map(|m| m.as_str().to_owned())
}
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let parts = |s: &str| {
        s.split('.')
            .map(|n| n.parse::<u64>().unwrap_or(0))
            .take(3)
            .collect::<Vec<_>>()
    };
    parts(a).cmp(&parts(b))
}
fn cwd() -> PathBuf {
    crate::accounts::home().unwrap_or_else(|| PathBuf::from("/"))
}
fn installed(kind: Kind) -> Result<String, String> {
    let path = provider::resolve(kind).ok_or("CLI could not be resolved")?;
    let output = cli::output(
        &path,
        &["--version".into()],
        &cwd(),
        Duration::from_secs(10),
    )?;
    parse_version(&output).ok_or("CLI returned no version".into())
}
pub fn latest_version(kind: Kind) -> Result<String, String> {
    let package = package(kind).ok_or("No update feed for harness")?;
    let text = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .build()
        .get(&format!("https://registry.npmjs.org/{package}/latest"))
        .set("User-Agent", "vorkspace")
        .set("Accept", "application/json")
        .call()
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|e| e.to_string())?;
    let body: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    parse_version(body["version"].as_str().unwrap_or(""))
        .ok_or("npm registry returned no version".into())
}
/// Hidden and unresolved providers are excluded; failures are deliberately silent.
pub fn find_updates(hidden: &[String]) -> Vec<Update> {
    std::thread::scope(|scope| {
        let jobs = Kind::ALL
            .into_iter()
            .filter(|k| package(*k).is_some() && !hidden.iter().any(|id| id == k.id()))
            .map(|harness| {
                scope.spawn(move || {
                    let installed = installed(harness).ok()?;
                    let latest = latest_version(harness).ok()?;
                    (compare_versions(&latest, &installed).is_gt()).then_some(Update {
                        harness,
                        installed,
                        latest,
                    })
                })
            })
            .collect::<Vec<_>>();
        jobs.into_iter()
            .filter_map(|job| job.join().ok().flatten())
            .collect()
    })
}
/// Homebrew installs are updated by Homebrew; `<cli> update` only points at it
/// and exits successfully without changing the version.
fn homebrew_upgrade(resolved: &std::path::Path) -> Option<Vec<String>> {
    let real = std::fs::canonicalize(resolved).ok()?;
    let parts: Vec<_> = real
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    if let Some(at) = parts.iter().position(|part| part == "Caskroom") {
        let cask = parts.get(at + 1)?;
        return Some(vec![
            "upgrade".into(),
            "--cask".into(),
            "--greedy".into(),
            cask.clone(),
        ]);
    }
    let at = parts.iter().position(|part| part == "Cellar")?;
    Some(vec!["upgrade".into(), parts.get(at + 1)?.clone()])
}
/// Use the installer that owns the resolved CLI, then verify its actual version
/// and refresh models before reporting success (exit status alone is insufficient).
pub fn run_update(update: &Update) -> Result<String, String> {
    let args: &[&str] = match update.harness {
        Kind::Claude | Kind::Codex => &["update"],
        Kind::Pi => &["update", "--self"],
        _ => return Err("No updater for harness".into()),
    };
    let path = provider::resolve(update.harness).ok_or("CLI could not be resolved")?;
    let (program, args, homebrew) = match homebrew_upgrade(&path) {
        Some(upgrade) => (
            crate::scm::shell::resolve_gui_binary("brew").ok_or("Homebrew could not be found")?,
            upgrade,
            true,
        ),
        None => (path, args.iter().map(|s| s.to_string()).collect(), false),
    };
    cli::output(&program, &args, &cwd(), Duration::from_secs(300)).map_err(|error| {
        error
            .lines()
            .rfind(|l| !l.trim().is_empty())
            .unwrap_or("Update failed")
            .to_owned()
    })?;
    let version = installed(update.harness).unwrap_or_else(|_| update.installed.clone());
    if compare_versions(&version, &update.latest).is_lt() {
        return Err(if homebrew {
            format!(
                "Still on {version}. Homebrew has not published {} yet.",
                update.latest
            )
        } else {
            format!("Still on {version} after updating.")
        });
    }
    catalog::invalidate(update.harness, &crate::accounts::Profile::default());
    catalog::models(update.harness)?;
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn homebrew_installs_are_upgraded_through_brew() {
        let root = std::env::temp_dir().join(format!("vorkspace-brew-{}", crate::bot::new_id()));
        let cask = root.join("Caskroom/claude-code@latest/2.1.311");
        let formula = root.join("Cellar/codex/1.2.3/bin");
        let plain = root.join("bin");
        for dir in [&cask, &formula, &plain] {
            std::fs::create_dir_all(dir).unwrap();
        }
        for file in [
            cask.join("claude"),
            formula.join("codex"),
            plain.join("claude"),
        ] {
            std::fs::write(&file, "").unwrap();
        }
        // Homebrew exposes cask binaries through a symlink on the PATH.
        let link = root.join("claude-link");
        std::os::unix::fs::symlink(cask.join("claude"), &link).unwrap();
        assert_eq!(
            homebrew_upgrade(&link).unwrap(),
            ["upgrade", "--cask", "--greedy", "claude-code@latest"]
        );
        assert_eq!(
            homebrew_upgrade(&formula.join("codex")).unwrap(),
            ["upgrade", "codex"]
        );
        assert!(homebrew_upgrade(&plain.join("claude")).is_none());
        assert!(homebrew_upgrade(&root.join("missing")).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
