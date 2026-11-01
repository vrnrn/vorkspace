//! Inbox arrival/revision sounds use the same saved project policy as sessions.
use super::delivery::ProjectRule;
use vorkspace_core::inbox::Item;
use vorkspace_ui::preferences::Preferences;

pub(crate) fn should_chime(
    preferences: &Preferences,
    connectors: &vorkspace_core::inbox::Connectors,
    changed: &[Item],
    enabled_at: u64,
    now: u64,
) -> bool {
    preferences.sounds
        && changed.iter().any(|item| {
            let occurred = item.updated_ms().max(0) as u64;
            connectors.enabled(&item.provider)
                && occurred >= enabled_at
                && ProjectRule::from_preferences(
                    &preferences.project_notifications,
                    &item.notification_project().id,
                    if item.kind == "pr" {
                        "pullRequests"
                    } else {
                        "issues"
                    },
                )
                .allows(occurred, now)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn muted_projects_and_old_changes_do_not_consume_the_batches_single_cue() {
        let mut prefs = Preferences::default();
        let connectors = vorkspace_core::inbox::Connectors::default();
        let item = Item {
            provider: "github".into(),
            kind: "pr".into(),
            project_path: "/first".into(),
            updated_at: "2026-10-22T10:00:00Z".into(),
            ..Default::default()
        };
        let second = Item {
            project_path: "/second".into(),
            ..item.clone()
        };
        let at = item.updated_ms() as u64;
        prefs.project_notifications = json!({"local:/first":{"disabled":["pullRequests"]}});
        assert!(!should_chime(
            &prefs,
            &connectors,
            std::slice::from_ref(&item),
            0,
            at + 1
        ));
        assert!(should_chime(
            &prefs,
            &connectors,
            &[item.clone(), second.clone()],
            0,
            at + 1
        ));
        prefs.project_notifications["local:/second"] = json!({"mutedUntil":null});
        assert!(!should_chime(
            &prefs,
            &connectors,
            &[item.clone(), second.clone()],
            0,
            at + 1
        ));
        prefs.project_notifications = json!({"local:/first":{"resumedAt":at+1}});
        assert!(!should_chime(
            &prefs,
            &connectors,
            std::slice::from_ref(&item),
            0,
            at + 2
        ));
        assert!(!should_chime(
            &prefs,
            &connectors,
            std::slice::from_ref(&second),
            at + 1,
            at + 2
        ));
        let mut disabled = connectors.clone();
        disabled.set("github", false).unwrap();
        assert!(!should_chime(
            &prefs,
            &disabled,
            std::slice::from_ref(&second),
            0,
            at + 2
        ));
        prefs.sounds = false;
        assert!(!should_chime(
            &prefs,
            &connectors,
            std::slice::from_ref(&second),
            0,
            at + 2
        ));
    }
}
