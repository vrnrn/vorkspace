#[test]
fn starter_skills_validate_and_never_replace_existing_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = super::create_blank(dir.path().to_str().unwrap(), "My Skill", false).unwrap();
    assert!(path.ends_with(".agents/skills/my-skill/SKILL.md"));
    let before = std::fs::read_to_string(&path).unwrap();
    assert!(before.contains("description: My Skill. Use when the user asks to my skill."));
    assert!(super::create_blank(dir.path().to_str().unwrap(), "my-skill", false).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    assert!(super::create_blank(dir.path().to_str().unwrap(), "!!!", false).is_err());
}

use super::*;
use std::io::ErrorKind;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

struct Tmp(PathBuf);
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tmp(label: &str) -> Tmp {
    loop {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "vorkspace-skills-{label}-{}-{stamp}-{seq}",
            std::process::id()
        ));
        match std::fs::create_dir(&dir) {
            Ok(()) => return Tmp(dir),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("{}", error),
        }
    }
}

fn write_skill(root: &Path, folder: &str, body: &str) {
    let dir = root.join(folder);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), body).unwrap();
}

fn write_plugin_setting(root: &Path, file: &str, plugin_id: &str, enabled: bool) {
    let dir = root.join(".claude");
    std::fs::create_dir_all(&dir).unwrap();
    let settings = serde_json::json!({
        "enabledPlugins": { plugin_id: enabled }
    });
    std::fs::write(dir.join(file), serde_json::to_vec(&settings).unwrap()).unwrap();
}

#[test]
fn parse_frontmatter_reads_name_and_folded_description() {
    let (name, desc) = parse_frontmatter(
            "---\nname: review-pr\ndescription: >\n  Review pull requests.\n  Use when asked to review.\n---\n\n# hi\n",
            "fallback",
        );
    assert_eq!(name, "review-pr");
    assert_eq!(desc, "Review pull requests. Use when asked to review.");
}

#[test]
fn parse_frontmatter_falls_back_to_folder_name() {
    let (name, desc) = parse_frontmatter("# no yaml\n", "create-skill");
    assert_eq!(name, "create-skill");
    assert_eq!(desc, "");
}

#[test]
fn utf8_prefix_drops_only_a_cut_character() {
    // 2-byte, 3-byte and 4-byte characters, with each possible cut.
    for text in ["abж", "ab€", "ab😀"] {
        let bytes = text.as_bytes();
        let last = text.chars().last().unwrap().len_utf8();
        for removed in 1..last {
            let cut = bytes[..bytes.len() - removed].to_vec();
            assert_eq!(utf8_prefix(cut).as_deref(), Some("ab"));
        }
        assert_eq!(utf8_prefix(bytes.to_vec()).as_deref(), Some(text));
    }
    assert_eq!(utf8_prefix(vec![b'a', 0xff, b'b']), None);
}

#[test]
fn lists_large_skill_when_prefix_cuts_a_character() {
    let project = tmp("proj");
    let home = tmp("home");
    let mut body = String::from("---\nname: big-cyrillic\ndescription: Big skill\n---\n");
    // Put the first byte of a 2-byte character at the last byte of the prefix.
    while body.len() < MAX_FRONTMATTER_BYTES - 1 {
        body.push('x');
    }
    body.push_str(&"ж".repeat(1_000));
    write_skill(&home.0.join(".agents/skills"), "big-cyrillic", &body);

    let skills = list_skills_from(&project.0, Some(&home.0), None);
    let skill = skills.iter().find(|s| s.name == "big-cyrillic").unwrap();
    assert_eq!(skill.description, "Big skill");
}

#[test]
fn agents_skills_win_over_provider_dirs() {
    let project = tmp("proj");
    let home = tmp("home");
    write_skill(
        &project.0.join(".agents/skills"),
        "ship",
        "---\nname: ship\ndescription: vorkspace ship\n---\n",
    );
    write_skill(
        &project.0.join(".claude/skills"),
        "ship",
        "---\nname: ship\ndescription: Claude ship\n---\n",
    );
    write_skill(
        &home.0.join(".agents/skills"),
        "greet",
        "---\nname: greet\ndescription: Hello\n---\n",
    );
    write_skill(
        &project.0.join(".cursor/skills"),
        "retired-only",
        "---\nname: retired-only\ndescription: Retired integration\n---\n",
    );

    let skills = list_skills_from(&project.0, Some(&home.0), None);
    let ship = skills.iter().find(|s| s.name == "ship").unwrap();
    assert_eq!(ship.description, "vorkspace ship");
    assert_eq!(ship.source, "agents");
    assert_eq!(ship.scope, "project");

    let greet = skills.iter().find(|s| s.name == "greet").unwrap();
    assert_eq!(greet.scope, "user");
    assert_eq!(greet.source, "agents");

    assert!(skills.iter().all(|skill| skill.name != "retired-only"));
}

#[test]
fn lists_large_catalogs_from_every_root() {
    let project = tmp("proj");
    let home = tmp("home");
    for index in 0..1_400 {
        let name = format!("shared-{index:04}");
        write_skill(
            &home.0.join(".agents/skills"),
            &name,
            &format!("---\nname: {name}\ndescription: Shared skill {index}\n---\n"),
        );
    }
    write_skill(
        &home.0.join(".codex/skills"),
        "codex-only",
        "---\nname: codex-only\ndescription: Codex native\n---\n",
    );

    let skills = list_skills_from(&project.0, Some(&home.0), None);
    assert_eq!(skills.len(), 1_401);
    assert!(skills.iter().any(|s| s.name == "shared-1399"));
    let codex = skills.iter().find(|s| s.name == "codex-only").unwrap();
    assert_eq!(codex.source, "codex");
}

#[test]
fn discovers_pi_project_and_user_skills() {
    let project = tmp("proj-pi");
    let home = tmp("home-pi");
    write_skill(
        &project.0.join(".pi/skills"),
        "pi-review",
        "---\nname: pi-review\ndescription: Pi project skill\n---\n",
    );
    write_skill(
        &home.0.join(".pi/agent/skills"),
        "pi-global",
        "---\nname: pi-global\ndescription: Pi user skill\n---\n",
    );

    let skills = list_skills_from(&project.0, Some(&home.0), None);
    let project_skill = skills.iter().find(|s| s.name == "pi-review").unwrap();
    assert_eq!(project_skill.source, "pi");
    assert_eq!(project_skill.scope, "project");
    let user_skill = skills.iter().find(|s| s.name == "pi-global").unwrap();
    assert_eq!(user_skill.source, "pi");
    assert_eq!(user_skill.scope, "user");
}

#[test]
fn discovers_antigravity_user_skills() {
    let project = tmp("proj-agy");
    let home = tmp("home-agy");
    write_skill(
        &home.0.join(".gemini/antigravity/skills"),
        "agy-review",
        "---\nname: agy-review\ndescription: Antigravity user skill\n---\n",
    );
    let skills = list_skills_from(&project.0, Some(&home.0), None);
    let agy = skills.iter().find(|s| s.name == "agy-review").unwrap();
    assert_eq!(agy.source, "antigravity");
    assert_eq!(agy.scope, "user");
}

#[test]
fn antigravity_skills_do_not_shadow_existing_providers() {
    let project = tmp("proj-agy-shadow");
    let home = tmp("home-agy-shadow");
    for (root, desc) in [
        (home.0.join(".omp/agent/skills"), "OMP agent skill"),
        (
            home.0.join(".gemini/antigravity/skills"),
            "Antigravity user skill",
        ),
    ] {
        write_skill(
            &root,
            "shared-name",
            &format!("---\nname: shared-name\ndescription: {desc}\n---\n"),
        );
    }
    let skills = list_skills_from(&project.0, Some(&home.0), None);
    let skill = skills.iter().find(|s| s.name == "shared-name").unwrap();
    assert_eq!(skill.description, "OMP agent skill");
    assert_eq!(skill.source, "omp");
}

#[test]
fn discovers_installed_claude_plugin_skills() {
    let project = tmp("proj-claude-plugin");
    let home = tmp("home-claude-plugin");
    let plugin = home
        .0
        .join(".claude/plugins/cache/community/workflow-kit/1.2.3");
    write_skill(
        &plugin.join("skills"),
        "quick-plan",
        "---\nname: quick-plan\ndescription: Plan from plugin\n---\n",
    );
    write_skill(
        &home.0.join(".claude/skills"),
        "quick-plan",
        "---\nname: quick-plan\ndescription: Personal plan\n---\n",
    );
    std::fs::create_dir_all(home.0.join(".claude/plugins")).unwrap();
    std::fs::write(
            home.0.join(".claude/plugins/installed_plugins.json"),
            r#"{"version":2,"plugins":{"workflow-kit@community":[{"scope":"user","installPath":"~/.claude/plugins/cache/community/workflow-kit/1.2.3","version":"1.2.3"}]}}"#,
        )
        .unwrap();

    let skills = list_skills_from(&project.0, Some(&home.0), None);
    let skill = skills
        .iter()
        .find(|skill| skill.name == "workflow-kit:quick-plan")
        .unwrap();
    assert_eq!(skill.description, "Plan from plugin");
    assert_eq!(skill.source, "claude");
    assert_eq!(skill.scope, "user");
    assert!(skill
        .path
        .ends_with("workflow-kit/1.2.3/skills/quick-plan/SKILL.md"));
    assert!(skills.iter().any(|skill| skill.name == "quick-plan"));
}

#[test]
fn claude_project_plugins_only_apply_to_their_project() {
    let project = tmp("proj-claude-scoped");
    let other = tmp("other-claude-scoped");
    let home = tmp("home-claude-scoped");
    let plugin = home
        .0
        .join(".claude/plugins/cache/community/workflow-kit/2.0.0");
    write_skill(
        &plugin.join("skills"),
        "feature-delivery",
        "---\nname: feature-delivery\ndescription: Deliver feature\n---\n",
    );
    std::fs::create_dir_all(home.0.join(".claude/plugins")).unwrap();
    let registry = serde_json::json!({
        "version": 2,
        "plugins": {
            "workflow-kit@community": [{
                "scope": "project",
                "projectPath": project.0.to_string_lossy(),
                "installPath": plugin.to_string_lossy(),
                "version": "2.0.0"
            }]
        }
    });
    std::fs::write(
        home.0.join(".claude/plugins/installed_plugins.json"),
        serde_json::to_vec(&registry).unwrap(),
    )
    .unwrap();

    let nested = project.0.join("src");
    std::fs::create_dir_all(&nested).unwrap();
    let matching = list_skills_from(&nested, Some(&home.0), None);
    let skill = matching
        .iter()
        .find(|skill| skill.name == "workflow-kit:feature-delivery")
        .unwrap();
    assert_eq!(skill.scope, "project");

    let unrelated = list_skills_from(&other.0, Some(&home.0), None);
    assert!(!unrelated
        .iter()
        .any(|skill| skill.name == "workflow-kit:feature-delivery"));
}

#[test]
fn disabled_claude_plugin_skills_are_hidden() {
    let project = tmp("proj-claude-disabled");
    let home = tmp("home-claude-disabled");
    let plugin = home
        .0
        .join(".claude/plugins/cache/community/workflow-kit/1.2.3");
    write_skill(
        &plugin.join("skills"),
        "quick-plan",
        "---\nname: quick-plan\ndescription: Plan from plugin\n---\n",
    );
    std::fs::create_dir_all(home.0.join(".claude/plugins")).unwrap();
    std::fs::write(
            home.0.join(".claude/plugins/installed_plugins.json"),
            r#"{"version":2,"plugins":{"workflow-kit@community":[{"scope":"user","installPath":"~/.claude/plugins/cache/community/workflow-kit/1.2.3","version":"1.2.3"}]}}"#,
        )
        .unwrap();
    write_plugin_setting(&home.0, "settings.json", "workflow-kit@community", false);

    let skills = list_skills_from(&project.0, Some(&home.0), None);
    assert!(!skills
        .iter()
        .any(|skill| skill.name == "workflow-kit:quick-plan"));
}

#[test]
fn claude_plugin_enablement_uses_local_project_user_precedence() {
    let project = tmp("proj-claude-precedence");
    let nested = project.0.join("src");
    let home = tmp("home-claude-precedence");
    std::fs::create_dir_all(&nested).unwrap();
    write_plugin_setting(&home.0, "settings.json", "workflow-kit@community", false);
    write_plugin_setting(&project.0, "settings.json", "workflow-kit@community", true);
    assert!(claude_plugin_enabled(
        &home.0,
        &nested,
        "workflow-kit@community"
    ));
    write_plugin_setting(
        &project.0,
        "settings.local.json",
        "workflow-kit@community",
        false,
    );
    assert!(!claude_plugin_enabled(
        &home.0,
        &nested,
        "workflow-kit@community"
    ));
}

#[test]
fn managed_plugin_settings_apply_dropins_in_order() {
    let root = tmp("managed-settings");
    let plugin_id = "workflow-kit@community";
    let base = serde_json::json!({ "enabledPlugins": { plugin_id: true } });
    std::fs::write(
        root.0.join("managed-settings.json"),
        serde_json::to_vec(&base).unwrap(),
    )
    .unwrap();
    let dropins = root.0.join("managed-settings.d");
    std::fs::create_dir_all(&dropins).unwrap();
    let earlier = serde_json::json!({ "enabledPlugins": { plugin_id: true } });
    let later = serde_json::json!({ "enabledPlugins": { plugin_id: false } });
    std::fs::write(
        dropins.join("10-enable.json"),
        serde_json::to_vec(&earlier).unwrap(),
    )
    .unwrap();
    std::fs::write(
        dropins.join("20-disable.json"),
        serde_json::to_vec(&later).unwrap(),
    )
    .unwrap();

    assert_eq!(
        managed_plugin_setting_from_root(&root.0, plugin_id),
        Some(false)
    );
}

#[test]
fn path_is_within_fails_closed_when_either_path_is_missing() {
    let root = tmp("path-root");
    let nested = root.0.join("src");
    std::fs::create_dir_all(&nested).unwrap();
    assert!(path_is_within(&nested, &root.0));
    assert!(!path_is_within(&root.0.join("missing"), &root.0));
    assert!(!path_is_within(&nested, &root.0.join("missing")));
}

#[test]
fn ignores_unregistered_claude_plugin_cache_versions() {
    let project = tmp("proj-claude-stale");
    let home = tmp("home-claude-stale");
    let stale = home
        .0
        .join(".claude/plugins/cache/community/workflow-kit/0.9.0");
    write_skill(
        &stale.join("skills"),
        "stale-skill",
        "---\nname: stale-skill\ndescription: Old cached skill\n---\n",
    );
    std::fs::create_dir_all(home.0.join(".claude/plugins")).unwrap();
    std::fs::write(
        home.0.join(".claude/plugins/installed_plugins.json"),
        r#"{"version":2,"plugins":{}}"#,
    )
    .unwrap();

    let skills = list_skills_from(&project.0, Some(&home.0), None);
    assert!(!skills.iter().any(|skill| skill.name == "stale-skill"));
}

#[test]
fn skips_dirs_without_skill_md() {
    let project = tmp("empty");
    std::fs::create_dir_all(project.0.join(".agents/skills/nope")).unwrap();
    let skills = list_skills_from(&project.0, None, None);
    assert!(skills.is_empty());
}
#[test]
fn disabled_project_skill_falls_back_to_same_name_personal_skill() {
    let project = tmp("proj-fallback");
    let home = tmp("home-fallback");
    write_skill(
        &project.0.join(".agents/skills"),
        "review",
        "---\nname: review\ndescription: Project review\n---\n",
    );
    write_skill(
        &home.0.join(".agents/skills"),
        "review",
        "---\nname: review\ndescription: Personal review\n---\n",
    );

    let project_skill_path =
        crate::shell_env::path_to_js(&project.0.join(".agents/skills/review/SKILL.md"));
    let personal_skill_path =
        crate::shell_env::path_to_js(&home.0.join(".agents/skills/review/SKILL.md"));

    // 1. When neither is disabled, project skill wins.
    let enabled_skills = list_skills_from(&project.0, Some(&home.0), None);
    let review = enabled_skills.iter().find(|s| s.name == "review").unwrap();
    assert_eq!(review.description, "Project review");
    assert_eq!(review.path, project_skill_path);
    assert_eq!(review.scope, "project");

    // 2. When only project skill is disabled, personal skill is active fallback.
    let fallback_skills = list_skills_from(
        &project.0,
        Some(&home.0),
        Some(std::slice::from_ref(&project_skill_path)),
    );
    let review = fallback_skills.iter().find(|s| s.name == "review").unwrap();
    assert_eq!(review.description, "Personal review");
    assert_eq!(review.path, personal_skill_path);
    assert_eq!(review.scope, "user");

    // 3. When project skill is re-enabled, project skill wins again.
    let restored_skills = list_skills_from(&project.0, Some(&home.0), Some(&[]));
    let review = restored_skills.iter().find(|s| s.name == "review").unwrap();
    assert_eq!(review.description, "Project review");
    assert_eq!(review.path, project_skill_path);

    // 4. When both are disabled, skill is omitted from catalog.
    let none_skills = list_skills_from(
        &project.0,
        Some(&home.0),
        Some(&[project_skill_path.clone(), personal_skill_path.clone()]),
    );
    assert!(none_skills.iter().all(|s| s.name != "review"));

    // 5. When lower-priority personal skill is disabled, project winner is unaffected.
    let winner_skills = list_skills_from(
        &project.0,
        Some(&home.0),
        Some(std::slice::from_ref(&personal_skill_path)),
    );
    let review = winner_skills.iter().find(|s| s.name == "review").unwrap();
    assert_eq!(review.description, "Project review");
    assert_eq!(review.path, project_skill_path);
}

#[test]
fn disabled_skill_matching_preserves_distinct_unix_paths_with_backslashes() {
    let project = tmp("proj-unix-bs");
    let skill_with_bs_dir = project.0.join(".agents/skills/a\\b");
    write_skill(
        skill_with_bs_dir.parent().unwrap(),
        "a\\b",
        "---\nname: slash-skill\ndescription: Slash skill\n---\n",
    );
    let nested_file = project.0.join(".agents/skills/a/b/SKILL.md");
    std::fs::create_dir_all(nested_file.parent().unwrap()).unwrap();
    std::fs::write(
        &nested_file,
        "---\nname: nested-skill\ndescription: Nested\n---\n",
    )
    .unwrap();

    assert!(skill_with_bs_dir.join("SKILL.md").exists());
    assert!(nested_file.exists());

    let nested_path = crate::shell_env::path_to_js(&nested_file);
    let skills = list_skills_from(&project.0, None, Some(&[nested_path]));
    let slash_skill = skills.iter().find(|s| s.name == "slash-skill");
    assert!(
        slash_skill.is_some(),
        "distinct backslash path on Unix must not be disabled by colliding slash path"
    );
}
#[test]
fn retired_provider_skill_roots_are_not_discovered() {
    let project = tmp("retired-skills-project");
    let home = tmp("retired-skills-home");
    for provider in ["cursor", "opencode", "grok", "fx", "hermes", "devin"] {
        for root in [&project.0, &home.0] {
            write_skill(
                &root.join(format!(".{provider}/skills")),
                provider,
                &format!("---\nname: {provider}\ndescription: retired provider\n---\n"),
            );
        }
    }
    write_skill(
        &home.0.join(".config/devin/skills"),
        "retired-global",
        "---\nname: retired-global\n---\n",
    );
    assert!(list_skills_from(&project.0, Some(&home.0), None).is_empty());
}
