use url::Url;

/// The display URL identifies the instance. Requests still use the connected
/// API base and a validated item identity.
pub(super) fn on_instance(base: &str, item_url: &str) -> Option<(Url, Url)> {
    let base = Url::parse(base).ok()?;
    let item = Url::parse(item_url).ok()?;
    (matches!(base.scheme(), "https" | "http")
        && item.origin() == base.origin()
        && item.username().is_empty()
        && item.password().is_none())
    .then_some((base, item))
}

pub(super) fn item_url(base: &Url, path: impl IntoIterator<Item = impl AsRef<str>>) -> Url {
    let mut url = base.clone();
    url.path_segments_mut()
        .expect("HTTP instance has path segments")
        .pop_if_empty()
        .extend(path);
    url
}

#[cfg(test)]
mod tests {
    use crate::inbox::{Item, Service};
    use serde_json::json;
    use std::fs;

    #[test]
    fn persisted_items_cannot_switch_the_instance_used_for_reads_or_writes() {
        let data = tempfile::tempdir().unwrap();
        let service = Service {
            data_dir: data.path().into(),
            cwd: data.path().into(),
        };
        for (provider, config, old_url, hint) in [
            (
                "jira",
                json!({"site":"https://new.example","email":"test@example.com","token":"fixture"}),
                "https://old.example/browse/DEMO-42",
                "this issue's Jira site",
            ),
            (
                "gitlab",
                json!({"url":"https://gitlab.example/current","token":"fixture"}),
                "https://gitlab.example/older/acme/project/-/merge_requests/42",
                "this item's GitLab instance",
            ),
        ] {
            fs::write(
                data.path().join(format!("{provider}-config.json")),
                config.to_string(),
            )
            .unwrap();
            let item = Item {
                provider: provider.into(),
                kind: if provider == "jira" { "issue" } else { "pr" }.into(),
                identifier: "DEMO-42".into(),
                url: old_url.into(),
                repo: "acme/project".into(),
                number: 42,
                ..Default::default()
            };
            assert!(matches!(service.discussion(&item), Err(error) if error.contains(hint)));
            assert!(service
                .comment(&item, "draft".into(), String::new())
                .unwrap_err()
                .contains(hint));
            if provider == "gitlab" {
                assert!(service.diff(&item).unwrap_err().contains(hint));
            }
        }
    }
}
