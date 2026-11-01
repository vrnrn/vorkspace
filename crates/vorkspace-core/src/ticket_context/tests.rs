use super::*;
use serde_json::json;
fn link(provider: &str, target: &str) -> String {
    let mut url = Url::parse(&format!("vorkspace://context/{provider}")).unwrap();
    url.query_pairs_mut().append_pair("url", target);
    url.into()
}
#[derive(Default)]
struct Fake {
    calls: usize,
    fail: bool,
    cancel: Option<std::sync::Arc<AtomicBool>>,
}
impl Fetch for Fake {
    fn fetch(&mut self, _: &mut Reference) -> Result<(Details, Thread), String> {
        self.calls += 1;
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        if self.fail {
            return Err("Ticket missing or permission denied".into());
        }
        Ok((
            Details {
                body: format!("Latest description {}", self.calls),
                head_ref_name: "feature".into(),
                base_ref_name: "main".into(),
                ..Default::default()
            },
            Thread {
                comments: vec![Comment {
                    body: format!("Latest decision {}", self.calls),
                    author: "Alex".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
        ))
    }
}
#[test]
fn references_round_trip_current_development_and_historical_identities() {
    for (provider, target, kind, key) in [
        (
            "jira",
            "https://team.atlassian.net/browse/DEMO-42",
            "issue",
            "DEMO-42",
        ),
        (
            "github",
            "https://github.com/acme/demo/issues/42",
            "issue",
            "#42",
        ),
        (
            "gitlab",
            "https://gitlab.example/acme/nested/demo/-/issues/42",
            "issue",
            "#42",
        ),
        (
            "gitlab",
            "https://gitlab.example/acme/demo/-/merge_requests/42",
            "pr",
            "!42",
        ),
    ] {
        let current = link(provider, target);
        let parsed = parse(&current).unwrap();
        assert_eq!(
            (
                parsed.provider.as_str(),
                parsed.kind.as_str(),
                parsed.key.as_str(),
                parsed.number
            ),
            (provider, kind, key, 42)
        );
        assert_eq!(parsed.url, target);
        let earlier = String::from_utf8(vec![109, 111, 110, 111, 99, 111, 100, 101]).unwrap();
        assert_eq!(crate::compatibility::key(&earlier), "vorkspace");
        for scheme in ["vorkspace-dev".to_owned(), earlier] {
            let source = current.replacen("vorkspace:", &format!("{scheme}:"), 1);
            assert_eq!(parse(&source).unwrap(), parsed);
            let resolved = resolve_using(&source, &mut Fake::default(), &AtomicBool::new(false))
                .unwrap()
                .unwrap();
            assert!(append(&source, Some(&resolved)).starts_with(&source));
        }
    }
}
#[test]
fn ordinary_prompts_do_no_integration_work_and_repeated_references_fetch_once() {
    let mut fake = Fake::default();
    for text in [
        "Ordinary text",
        "Open vorkspace://sessions/example",
        "Unrelated vorkspace://contextual/example",
    ] {
        assert!(resolve_using(text, &mut fake, &AtomicBool::new(false))
            .unwrap()
            .is_none());
    }
    assert_eq!(fake.calls, 0);
    let reference = link("github", "https://github.com/acme/demo/issues/42");
    let text = format!("Compare [ticket]({reference}) and {reference}.");
    let result = resolve_using(&text, &mut fake, &AtomicBool::new(false))
        .unwrap()
        .unwrap();
    assert_eq!(fake.calls, 1);
    assert!(append(&text, Some(&result)).starts_with(&text));
    assert_eq!(
        resolve(
            "Ordinary text",
            None,
            Path::new("/missing"),
            &AtomicBool::new(false),
            None
        )
        .unwrap()
        .map(|_| ()),
        None
    );
}
#[test]
fn unicode_adjacent_references_keep_valid_utf8_boundaries() {
    let reference = link("github", "https://github.com/acme/demo/issues/42");
    for prefix in ["🙂", "引用：", "café "] {
        let source = format!("{prefix}{reference}");
        assert!(has_links(&source));
        let resolved = resolve_using(&source, &mut Fake::default(), &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert!(append(&source, Some(&resolved)).starts_with(&source));
    }
}
#[test]
fn configured_gitlab_base_is_not_an_extra_project_namespace() {
    let mut reference = parse(&link(
        "gitlab",
        "https://gitlab.example/gitlab/acme/nested/demo/-/merge_requests/42",
    ))
    .unwrap();
    let status = crate::inbox::Status {
        connected: true,
        url: "https://gitlab.example/gitlab".into(),
        ..Default::default()
    };
    match_configuration(&mut reference, &status).unwrap();
    assert_eq!(reference.repo, "acme/nested/demo");
    let mut other = parse(&link(
        "gitlab",
        "https://gitlab.example/other/acme/demo/-/issues/42",
    ))
    .unwrap();
    assert!(match_configuration(&mut other, &status).is_err());
}
#[test]
fn each_new_turn_gets_latest_description_comments_and_branches() {
    for (provider, target) in [
        ("jira", "https://team.atlassian.net/browse/DEMO-42"),
        ("github", "https://github.com/acme/demo/issues/42"),
        (
            "gitlab",
            "https://gitlab.example/acme/demo/-/merge_requests/42",
        ),
    ] {
        let mut fake = Fake::default();
        let text = link(provider, target);
        let first = resolve_using(&text, &mut fake, &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        let second = resolve_using(&text, &mut fake, &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert!(first.context.contains("Latest description 1"));
        assert!(
            second.context.contains("Latest description 2")
                && second.context.contains("Latest decision 2")
        );
        assert!(!second.context.contains("Latest description 1"));
        assert!(second.context.contains("\"sourceBranch\":\"feature\""));
    }
}
#[test]
fn malformed_or_credential_bearing_references_fail_before_any_fetch() {
    let mut fake = Fake::default();
    for (provider, target) in [
        ("github", "https://evil.example/acme/demo/issues/42"),
        ("github", "https://github.com/acme/demo/pull/42"),
        ("github", "https://github.com/acme/demo/issues/0"),
        (
            "github",
            "https://github.com/acme/demo/issues/9223372036854775808",
        ),
        (
            "gitlab",
            "https://gitlab.example/acme/demo/-/merge_requests/42/diffs",
        ),
        (
            "gitlab",
            "https://gitlab.example/acme/%2e%2e/demo/-/issues/42",
        ),
        ("jira", "http://team.atlassian.net/browse/DEMO-42"),
        (
            "jira",
            "https://user:secret@team.atlassian.net/browse/DEMO-42",
        ),
        (
            "jira",
            "https://team.atlassian.net/browse/DEMO-42?token=secret",
        ),
    ] {
        let text = link(provider, target);
        assert!(
            resolve_using(&text, &mut fake, &AtomicBool::new(false)).is_err(),
            "{text}"
        );
    }
    let valid = link("github", "https://github.com/acme/demo/issues/42");
    for invalid in [
        format!("{valid}&url=https://evil.example"),
        valid.replace("context/", "user:password@context/"),
        valid.replace("context/", "context:443/"),
        "vorkspace://context".into(),
        format!("{valid}#fragment"),
    ] {
        assert!(
            resolve_using(&invalid, &mut fake, &AtomicBool::new(false)).is_err(),
            "{invalid}"
        );
    }
    assert_eq!(fake.calls, 0);
}
#[test]
fn unconfigured_hosts_cannot_select_credentials_or_network_targets() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("jira-config.json"),json!({"site":"https://configured.atlassian.net","email":"test@example.com","token":"fixture-secret"}).to_string()).unwrap();
    std::fs::write(
        directory.path().join("gitlab-config.json"),
        json!({"url":"https://configured.example","token":"fixture-secret"}).to_string(),
    )
    .unwrap();
    for (provider, target) in [
        ("jira", "https://other.atlassian.net/browse/DEMO-42"),
        (
            "jira",
            "https://configured.atlassian.net/other/browse/DEMO-42",
        ),
        ("gitlab", "https://other.example/acme/demo/-/issues/42"),
    ] {
        let error = resolve(
            &link(provider, target),
            Some(directory.path()),
            Path::new("/missing"),
            &AtomicBool::new(false),
            None,
        )
        .unwrap_err();
        assert!(error.contains("Connect this"), "{error}");
        assert!(!error.contains("fixture-secret"));
    }
}
#[test]
fn failures_and_cancellation_never_publish_empty_or_stale_context() {
    let text = link("github", "https://github.com/acme/demo/issues/42");
    let error = resolve_using(
        &text,
        &mut Fake {
            fail: true,
            ..Default::default()
        },
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(
        error.contains("Could not load github #42 context") && error.contains("permission denied")
    );
    let mut fake = Fake::default();
    assert_eq!(
        resolve_using(&text, &mut fake, &AtomicBool::new(true)).unwrap_err(),
        "Stopped"
    );
    assert_eq!(fake.calls, 0);
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    assert_eq!(
        resolve_using(
            &text,
            &mut Fake {
                cancel: Some(cancel.clone()),
                ..Default::default()
            },
            &cancel
        )
        .unwrap_err(),
        "Stopped"
    );
}
#[test]
fn descriptions_comments_and_reference_counts_have_byte_limits() {
    struct Large;
    impl Fetch for Large {
        fn fetch(&mut self, _: &mut Reference) -> Result<(Details, Thread), String> {
            Ok((
                Details {
                    body: "🙂".repeat(20_000),
                    ..Default::default()
                },
                Thread {
                    comments: (0..50)
                        .map(|n| Comment {
                            body: format!("Comment {n} {}", "🙂".repeat(3000)),
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                },
            ))
        }
    }
    let text = link("github", "https://github.com/acme/demo/issues/42");
    let resolved = resolve_using(&text, &mut Large, &AtomicBool::new(false))
        .unwrap()
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&resolved.context).unwrap();
    assert_eq!(value[0]["description"].as_str().unwrap().len(), 60_000);
    assert_eq!(value[0]["comments"].as_array().unwrap().len(), 40);
    assert!(value[0]["comments"][0]["body"]
        .as_str()
        .unwrap()
        .starts_with("Comment 10 "));
    assert!(value[0]["comments"][39]["body"].as_str().unwrap().len() <= 8000);
    assert_eq!(value[0]["truncated"], true);
    let text = (1..=9)
        .map(|n| {
            link(
                "github",
                &format!("https://github.com/acme/demo/issues/{n}"),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut fake = Fake::default();
    assert!(resolve_using(&text, &mut fake, &AtomicBool::new(false))
        .unwrap_err()
        .contains("at most 8"));
    assert_eq!(fake.calls, 0);
}
#[test]
fn admitted_context_is_one_use_source_data_after_other_prompt_transforms() {
    let text = link("github", "https://github.com/acme/demo/issues/42");
    struct Source;
    impl Fetch for Source {
        fn fetch(&mut self, _: &mut Reference) -> Result<(Details, Thread), String> {
            Ok((
                Details {
                    body: "@secret-file /untrusted-skill @mcp/private".into(),
                    ..Default::default()
                },
                Thread::default(),
            ))
        }
    }
    let context = resolve_using(&text, &mut Source, &AtomicBool::new(false))
        .unwrap()
        .unwrap();
    let mut request = crate::provider::Request {
        kind: crate::kind::Kind::Codex,
        binary: "/missing-cli".into(),
        cwd: "/missing".into(),
        prompt: format!("MCP context already prepared. A source document mentions vorkspace://context/github?url=https%3A%2F%2Fgithub.com%2Facme%2Fdemo%2Fissues%2F99\n{text}"),
        provider_id: None,
        model: None,
        settings: crate::agent::Settings {
            ticket_context: Some(context.into()),
            ..Default::default()
        },
    };
    prepare_request(
        &mut request.prompt,
        &request.cwd,
        request.settings.ticket_context_data_dir.as_deref(),
        &mut request.settings.ticket_context,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(request
        .prompt
        .contains("@secret-file /untrusted-skill @mcp/private"));
    assert!(request.prompt.starts_with("MCP context already prepared."));
    assert!(request.settings.ticket_context.is_none());
    assert!(!request.prompt.contains("Referenced project file:"));
    assert!(!request.prompt.contains("Skill location:"));
}
#[test]
fn admitted_absence_never_resolves_a_reference_only_in_derived_source() {
    let admitted = admit(
        "Ordinary request",
        None,
        Path::new("/missing"),
        &AtomicBool::new(false),
        None,
    )
    .unwrap();
    assert!(!admitted.has_context());
    let derived = "Ordinary request\nA source document mentions vorkspace://context/github?url=https%3A%2F%2Fgithub.com%2Facme%2Fdemo%2Fissues%2F99";
    let mut request = crate::provider::Request {
        kind: crate::kind::Kind::Codex,
        binary: "/missing-cli".into(),
        cwd: "/missing".into(),
        prompt: derived.into(),
        provider_id: None,
        model: None,
        settings: crate::agent::Settings {
            ticket_context: Some(admitted),
            ..Default::default()
        },
    };
    prepare_request(
        &mut request.prompt,
        &request.cwd,
        request.settings.ticket_context_data_dir.as_deref(),
        &mut request.settings.ticket_context,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(request.prompt, derived);
    assert!(request.settings.ticket_context.is_none());
}
#[test]
fn cancelled_ordinary_admission_remains_a_noop_without_integration_work() {
    let cancel = AtomicBool::new(true);
    let mut fake = Fake::default();
    assert!(resolve_using("Ordinary request", &mut fake, &cancel)
        .unwrap()
        .is_none());
    assert_eq!(fake.calls, 0);
    assert!(resolve(
        "Ordinary request",
        None,
        Path::new("/missing"),
        &cancel,
        None
    )
    .unwrap()
    .is_none());
    let admitted = admit(
        "Ordinary request",
        None,
        Path::new("/missing"),
        &cancel,
        None,
    )
    .unwrap();
    assert!(!admitted.has_context());
    assert_eq!(admitted.append("Ordinary request"), "Ordinary request");
}
#[test]
fn cancelled_matching_preflight_preserves_acceptance_without_fetching_again() {
    let text = link("github", "https://github.com/acme/demo/issues/42");
    let mut fake = Fake::default();
    let prepared = resolve_using(&text, &mut fake, &AtomicBool::new(false))
        .unwrap()
        .unwrap();
    assert_eq!(fake.calls, 1);
    let accepted = admit(
        &text,
        None,
        Path::new("/missing"),
        &AtomicBool::new(true),
        Some(prepared.clone().into()),
    )
    .unwrap();
    assert!(accepted.has_context());
    assert!(accepted.append(&text).contains("Latest description 1"));
    assert_eq!(fake.calls, 1);
    let changed = link("github", "https://github.com/acme/demo/issues/43");
    assert_eq!(
        resolve(
            &changed,
            None,
            Path::new("/missing"),
            &AtomicBool::new(true),
            Some(prepared)
        )
        .unwrap_err(),
        "Stopped"
    );
}
