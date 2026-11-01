
use super::*;

fn local_at(value: &str) -> i64 {
    let naive = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S").unwrap();
    Local
        .from_local_datetime(&naive)
        .earliest()
        .unwrap()
        .timestamp_millis()
}
fn save(service: &mut Service, triggers: Vec<AutomationTrigger>) -> Automation {
    let mut draft = AutomationUpsert::new("/repo".into(), "codex".into(), "model".into());
    draft.name = "Review work".into();
    draft.prompt = "Review the change and implement safe fixes.".into();
    draft.triggers = Some(triggers);
    draft.sync_triggers(now_millis());
    service.upsert(draft).unwrap()
}
fn item(number: i64) -> crate::inbox::Item {
    crate::inbox::Item {
        provider: "github".into(),
        kind: "pr".into(),
        repo: "acme/app".into(),
        project_path: "/repo".into(),
        number,
        title: format!("Change {number}"),
        url: format!("https://github.com/acme/app/pull/{number}"),
        created_at: "2026-09-19T10:00:00Z".into(),
        ..Default::default()
    }
}

#[test]
fn retired_connectors_cannot_be_saved_claimed_or_recovered_for_dispatch() {
    let directory = tempfile::tempdir().unwrap();
    let mut service = Service::open(&directory.path().join("sessions.sqlite")).unwrap();
    let supported = save(
        &mut service,
        vec![AutomationTrigger::new("jira", "issue_created")],
    );
    let now = now_millis();
    for provider in ["linear", "azuredevops"] {
        let mut draft = AutomationUpsert::from_automation(&supported);
        draft.triggers = Some(vec![AutomationTrigger::new(provider, "issue_created")]);
        draft.sync_triggers(now);
        assert_eq!(
            service.upsert(draft).unwrap_err(),
            "Invalid automation trigger."
        );
        let claim = AutomationEventClaim {
            event_key: format!("{provider}:issue:42"),
            event_kind: provider.into(),
            event: "issue_created".into(),
            scheduled_for: now,
            prompt: "Historical issue prompt".into(),
        };
        assert_eq!(
            service.claim_event(&supported.id, claim, now).unwrap_err(),
            "Invalid automation trigger."
        );
        let old_run = new_event_run(
            &supported.id,
            &format!("{provider}:issue:42"),
            provider,
            "issue_created",
            now,
            now,
            "Historical issue prompt",
        );
        write_run(&service.conn, &old_run).unwrap();
    }
    assert!(service.recover(now, now + 1).unwrap().is_empty());
    let history = service.runs(&supported.id).unwrap();
    assert_eq!(history.len(), 2);
    assert!(history.iter().all(|run| run.status == "cancelled"
        && run.error.as_deref() == Some("This connector is no longer supported.")));
    assert_eq!(service.list().unwrap().len(), 1);
}
#[test]
fn schedules_skip_weekends_and_advance_exact_occurrence_boundaries() {
    let hourly = AutomationTrigger {
        minute: 15,
        ..AutomationTrigger::new("time", "hourly")
    };
    assert_eq!(
        hourly.next_at(local_at("2026-09-19T10:20:00")),
        local_at("2026-09-19T11:15:00")
    );
    assert_eq!(
        hourly.next_at(local_at("2026-09-19T10:15:00")),
        local_at("2026-09-19T11:15:00")
    );
    let weekdays = AutomationTrigger::new("time", "weekdays");
    assert_eq!(
        weekdays.next_at(local_at("2026-09-18T10:00:00")),
        local_at("2026-09-21T09:00:00")
    );
    let daily = AutomationTrigger {
        time: "18:00".into(),
        ..AutomationTrigger::new("time", "daily")
    };
    assert_eq!(
        daily.next_at(local_at("2026-09-19T10:00:00")),
        local_at("2026-09-19T18:00:00")
    );
    assert_eq!(
        daily.next_at(local_at("2026-09-19T18:00:00")),
        local_at("2026-09-20T18:00:00")
    );
    assert_eq!(
        next_run_at(&[weekdays, daily], local_at("2026-09-19T10:00:00")),
        local_at("2026-09-19T18:00:00")
    );
}

#[test]
fn inbox_claims_filter_projects_repositories_and_actors_and_dedupe_across_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sessions.sqlite");
    let mut service = Service::open(&path).unwrap();
    let trigger = AutomationTrigger {
        repos: vec!["ACME/App".into()],
        ..AutomationTrigger::new("github", "pull_request_opened")
    };
    let automation = save(&mut service, vec![trigger]);
    let mut wrong = item(1);
    wrong.project_path = "/another".into();
    assert!(claim_inbox_events(&mut service, &[wrong], now_millis())
        .unwrap()
        .is_empty());
    let mut wrong = item(2);
    wrong.repo = "acme/other".into();
    assert!(claim_inbox_events(&mut service, &[wrong], now_millis())
        .unwrap()
        .is_empty());
    let claims = claim_inbox_events(&mut service, &[item(3), item(3)], now_millis()).unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(
        claims[0].run.event_key.as_deref(),
        Some("github:pr:acme/app:3")
    );
    assert!(claims[0]
        .run
        .prompt
        .as_deref()
        .unwrap()
        .contains("Work on this GitHub pull request:"));
    drop(service);
    let mut service = Service::open(&path).unwrap();
    assert!(claim_inbox_events(&mut service, &[item(3)], now_millis())
        .unwrap()
        .is_empty());
    let mut draft = AutomationUpsert::from_automation(&automation);
    draft.triggers.as_mut().unwrap()[0].actor = "someone".into();
    draft.sync_triggers(now_millis());
    service.upsert(draft).unwrap();
    assert!(claim_inbox_events(&mut service, &[item(4)], now_millis())
        .unwrap()
        .is_empty());
}

#[test]
fn failed_event_in_batch_keeps_successes_dispatchable_and_failed_item_retryable() {
    let directory = tempfile::tempdir().unwrap();
    let mut service = Service::open(&directory.path().join("sessions.sqlite")).unwrap();
    save(
        &mut service,
        vec![AutomationTrigger::new("github", "pull_request_opened")],
    );
    let mut invalid = item(2);
    invalid.url = "x".repeat(MAX_PROMPT + 10_001);
    let (claims, retry) =
        claim_inbox_events_retry(&mut service, &[item(1), invalid], now_millis()).unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(retry.len(), 1);
    assert_eq!(retry[0].number, 2);
    let (claims, retry) =
        claim_inbox_events_retry(&mut service, &[item(1), item(2)], now_millis()).unwrap();
    assert_eq!(claims.len(), 1);
    assert!(retry.is_empty());
    assert_eq!(
        claims[0].run.event_key.as_deref(),
        Some("github:pr:acme/app:2")
    );
}

#[test]
fn recovery_returns_pending_prompts_and_cancels_interrupted_running_runs() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sessions.sqlite");
    let mut service = Service::open(&path).unwrap();
    let automation = save(&mut service, vec![]);
    let now = now_millis();
    let pending = service.run_now(&automation.id, now).unwrap();
    let running = service.run_now(&automation.id, now + 1).unwrap();
    service
        .update_run(
            &running.id,
            "running",
            Some("session-1".into()),
            None,
            now + 2,
        )
        .unwrap();
    drop(service);
    let mut service = Service::open(&path).unwrap();
    let recovery = service.recover(now + 10, now + 20).unwrap();
    assert_eq!(recovery.len(), 1);
    assert_eq!(recovery[0].run.id, pending.id);
    assert_eq!(
        recovery[0].run.prompt.as_deref(),
        Some(automation.prompt.as_str())
    );
    let history = service.runs(&automation.id).unwrap();
    let interrupted = history.iter().find(|run| run.id == running.id).unwrap();
    assert_eq!(interrupted.status, "cancelled");
    assert_eq!(interrupted.session_id.as_deref(), Some("session-1"));
    assert!(interrupted
        .error
        .as_deref()
        .unwrap()
        .contains("Interrupted"));
}

#[test]
fn due_poll_preserves_successful_claims_when_a_later_transaction_fails() {
    let directory = tempfile::tempdir().unwrap();
    let mut service = Service::open(&directory.path().join("sessions.sqlite")).unwrap();
    let mut healthy = save(&mut service, vec![AutomationTrigger::new("time", "daily")]);
    let mut failing = save(&mut service, vec![AutomationTrigger::new("time", "daily")]);
    let now = now_millis();
    healthy.next_run_at = now - 1;
    failing.next_run_at = now - 1;
    healthy.updated_at = now + 1;
    failing.updated_at = now;
    write_automation(&service.conn, &healthy).unwrap();
    write_automation(&service.conn, &failing).unwrap();
    service.conn.execute_batch(&format!(
            "CREATE TRIGGER reject_one_automation_run BEFORE INSERT ON automation_runs
             WHEN NEW.automation_id = '{}' BEGIN SELECT RAISE(ABORT, 'temporary write failure'); END;",
            failing.id
        )).unwrap();

    let claimed = service.claim_due(now).unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].automation.id, healthy.id);
    assert_eq!(claimed[0].run.status, "pending");
    assert_eq!(
        get(&service.conn, &failing.id)
            .unwrap()
            .unwrap()
            .next_run_at,
        now - 1
    );
    assert!(service.runs(&failing.id).unwrap().is_empty());

    service
        .conn
        .execute_batch("DROP TRIGGER reject_one_automation_run;")
        .unwrap();
    let retried = service.claim_due(now).unwrap();
    assert_eq!(retried.len(), 1);
    assert_eq!(retried[0].automation.id, failing.id);
    assert_eq!(service.runs(&healthy.id).unwrap().len(), 1);
}
