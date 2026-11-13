use super::*;
use gpui::TestAppContext;
use vorkspace_core::{
    automations::{AutomationUpsert, DefinitionRevision},
    session_groups::parse_github_work_item,
    store::Store,
};

/// Reads the durable link fields of one conversation.
fn saved_link(path: &std::path::Path, persisted: &str) -> (Option<String>, Option<String>) {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT automation_id,linked_work_item_json FROM sessions WHERE id=?1",
            [persisted],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
}

struct Outcome {
    before: (Option<String>, Option<String>),
    after: (Option<String>, Option<String>),
    original: String,
    run_session: Option<String>,
}

/// Reuses a conversation linked to PR #1 for PR #2's run, optionally with
/// the user typing into it while the run's "running" write is in flight.
fn reuse_for_second_pull_request(cx: &mut TestAppContext, user_types: bool) -> Outcome {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("sessions.sqlite");
    let project = dir.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let project = project.to_str().unwrap().to_string();
    let store = Store::test(&database).unwrap();
    let mut service = Service::open(&database).unwrap();
    let mut draft = AutomationUpsert::new(project.clone(), "codex".into(), "model".into());
    draft.name = "Review pull requests".into();
    draft.prompt = "Review the pull request.".into();
    draft.reuse_session = true;
    draft.triggers = Some(Vec::new());
    let automation = service
        .save_definition(draft, DefinitionRevision::New)
        .unwrap();
    let original = store.create("codex", &project).unwrap();
    let first = parse_github_work_item("https://github.com/acme/app/pull/1").unwrap();
    assert!(
        store
            .link_automation_session(
                original.id,
                &automation.id,
                "codex",
                &project,
                None,
                Some(&first),
            )
            .unwrap()
    );
    let id = original.id;
    let persisted = store.persisted_session_id(id).unwrap();
    let before = saved_link(&database, &persisted);
    let queued = service
        .run_now(&automation.id, automation.updated_at, 1_000)
        .unwrap();
    // The second pull request's event run targets the same conversation.
    rusqlite::Connection::open(&database)
        .unwrap()
        .execute(
            "UPDATE automation_runs SET run_json=json_set(run_json,'$.trigger','event',\
             '$.eventKind','github','$.eventKey','github:pr:acme/app:2') WHERE id=?1",
            [&queued.run.id],
        )
        .unwrap();
    let due = service.claim_run(&queued.run.id, 1_000).unwrap().unwrap();
    assert!(automation_work_item(&due.run).is_some());
    let run_id = due.run.id.clone();
    let service = Arc::new(Mutex::new(service));
    let mut workspace = Workspace::default();
    workspace.add_project(&project);
    workspace.open(id);
    let saved = serde_json::to_string(&workspace).unwrap();
    let store = Arc::new(Mutex::new(store));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut shell = Shell::new(store, vec![original], project.clone(), Some(saved), cx);
                shell.pages.automation_state.service = Some(service.clone());
                shell.ensure_session(id, cx);
                shell
            })
        })
        .unwrap()
    });
    cx.run_until_parked();
    window
        .update(cx, |shell, window, cx| {
            let view = shell.sessions[&id].clone();
            assert!(view.read(cx).managed_availability(cx) == ManagedAvailability::Available);
            let candidate = Candidate {
                id,
                entity: view.entity_id(),
            };
            shell.pages.automation_state.pending.insert(run_id.clone());
            shell.pages.automation_state.reserved.insert(id);
            let launch = Launch {
                due,
                reveal: false,
                navigation: shell.navigation_revision,
            };
            // Reuse is accepted here; the run's "running" write is still queued.
            shell.finish_automation_preparation(launch, Some(candidate), Ok(None), window, cx);
            if user_types {
                let input = view.read(cx).input.clone();
                input.update(cx, |input, cx| input.set_text("My own message".into(), cx));
            }
        })
        .unwrap();
    cx.run_until_parked();
    let run = service
        .lock()
        .unwrap()
        .runs(&automation.id)
        .unwrap()
        .into_iter()
        .find(|run| run.id == run_id)
        .unwrap();
    Outcome {
        after: saved_link(&database, &persisted),
        before,
        original: persisted,
        run_session: run.session_id,
    }
}

#[gpui::test]
fn abandoned_reuse_keeps_the_original_conversation_link(cx: &mut TestAppContext) {
    let outcome = reuse_for_second_pull_request(cx, true);
    let moved = outcome.run_session.expect("the run has a conversation");
    assert_ne!(
        moved, outcome.original,
        "the run moved to a fresh conversation"
    );
    assert_eq!(outcome.after, outcome.before);
}

#[gpui::test]
fn accepted_reuse_links_the_conversation_to_the_new_run(cx: &mut TestAppContext) {
    let outcome = reuse_for_second_pull_request(cx, false);
    assert_eq!(outcome.run_session.as_ref(), Some(&outcome.original));
    assert_eq!(outcome.after.0, outcome.before.0);
    let second = parse_github_work_item("https://github.com/acme/app/pull/2").unwrap();
    let linked: vorkspace_core::session_groups::WorkItem =
        serde_json::from_str(outcome.after.1.as_deref().unwrap()).unwrap();
    assert_eq!(linked, second);
}
