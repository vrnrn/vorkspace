use super::*;

fn item(provider: &str, number: i64, title: &str) -> vorkspace_core::inbox::Item {
    vorkspace_core::inbox::Item {
        provider: provider.into(),
        kind: "issue".into(),
        repo: "team/repo".into(),
        number,
        title: title.into(),
        updated_at: "2026-11-01T10:00:00Z".into(),
        ..Default::default()
    }
}

#[test]
fn app_and_review_panes_share_negative_ids_and_items_keep_resource_identity() {
    let mut workspace = Workspace::default();
    workspace.review_owners.insert(-1, 17);
    let note = workspace
        .app_pane(
            AppPane::Note("note-a".into()),
            "/work".into(),
            "/work".into(),
        )
        .unwrap();
    assert_eq!(note, -2);
    let first = workspace
        .app_pane(
            AppPane::ConnectorItem(Box::new(item("github", 41, "Original title"))),
            "/work".into(),
            "/work".into(),
        )
        .unwrap();
    let same = workspace
        .app_pane(
            AppPane::ConnectorItem(Box::new(vorkspace_core::inbox::Item {
                title: "Updated title".into(),
                updated_at: "2026-11-02T10:00:00Z".into(),
                ..item("github", 41, "Original title")
            })),
            "/another".into(),
            "/another".into(),
        )
        .unwrap();
    assert_eq!(same, first);
    let AppPane::ConnectorItem(current) = &workspace.app_panes[&first].content else {
        panic!("connector descriptor lost its item");
    };
    assert_eq!(current.title, "Updated title");
    let second = workspace
        .app_pane(
            AppPane::ConnectorItem(Box::new(item("github", 42, "Another issue"))),
            "/work".into(),
            "/work".into(),
        )
        .unwrap();
    let other_provider = workspace
        .app_pane(
            AppPane::ConnectorItem(Box::new(item("gitlab", 41, "Other provider"))),
            "/work".into(),
            "/work".into(),
        )
        .unwrap();
    assert_ne!(first, second);
    assert_ne!(first, other_provider);
    let document = AppPane::BotDocument {
        bot: "bot-a".into(),
        document: "doc-a".into(),
    };
    let document_id = workspace
        .app_pane(document.clone(), "/work".into(), "/work".into())
        .unwrap();
    assert_eq!(
        workspace.app_pane(document, "/other".into(), "/other".into()),
        Some(document_id)
    );
    assert_ne!(
        workspace.app_pane(
            AppPane::BotDocument {
                bot: "bot-a".into(),
                document: "doc-b".into(),
            },
            "/work".into(),
            "/work".into(),
        ),
        Some(document_id)
    );
    let review = workspace.fresh_pane_id().unwrap();
    assert!(review < *workspace.app_panes.keys().min().unwrap());
    workspace.review_owners.insert(review, 17);
    assert!(workspace.fresh_pane_id().unwrap() < review);
}

#[test]
fn mixed_session_note_bot_terminal_and_document_layout_survives_restore() {
    let mut workspace = Workspace {
        project: "/work".into(),
        ..Default::default()
    };
    workspace.open(17);
    let note = workspace
        .app_pane(
            AppPane::Note("note-a".into()),
            "/work".into(),
            "/work".into(),
        )
        .unwrap();
    let bot = workspace
        .app_pane(AppPane::Bot("bot-a".into()), "/work".into(), "/work".into())
        .unwrap();
    let terminal = workspace
        .app_pane(
            AppPane::Terminal {
                id: 900,
                cwd: "/work".into(),
            },
            "/work".into(),
            "/work".into(),
        )
        .unwrap();
    let document = workspace
        .app_pane(
            AppPane::BotDocument {
                bot: "bot-a".into(),
                document: "doc-a".into(),
            },
            "/work".into(),
            "/work".into(),
        )
        .unwrap();
    workspace.split(note, SplitDir::Right);
    workspace.split(bot, SplitDir::Down);
    workspace.split(terminal, SplitDir::Right);
    workspace.split(document, SplitDir::Down);
    let leaves = workspace.layout(17).leaf_ids();
    assert_eq!(leaves.len(), 5);
    assert!(leaves.contains(&17));
    assert!(leaves.contains(&note));
    assert!(leaves.contains(&bot));
    assert!(leaves.contains(&terminal));
    assert!(leaves.contains(&document));
    workspace
        .scope_focus
        .insert("/work".into(), TitleTab::Workspace(17));
    // A stale leaf reference in title order must resolve to the mixed tab,
    // never reopen the same note as a second standalone tab.
    workspace.title_order.insert(0, TitleTab::Workspace(note));
    workspace
        .scope_focus
        .insert("stale".into(), TitleTab::Workspace(-999));
    let saved = serde_json::to_string(&workspace).unwrap();
    let mut restored = Workspace::restore(Some(&saved), &[(17, "/work".into())], "/work");
    assert_eq!(restored.tabs, [17]);
    assert_eq!(restored.title_order, [TitleTab::Workspace(17)]);
    assert_eq!(restored.layout(17).leaf_ids(), leaves);
    assert_eq!(restored.active, Some(document));
    assert_eq!(restored.tab_focus(17), document);
    assert_eq!(restored.scope_focus["/work"], TitleTab::Workspace(17));
    assert!(!restored.scope_focus.contains_key("stale"));
    assert!(restored.session_for_pane(note).is_none());
    assert_eq!(restored.session_for_pane(17), Some(17));

    assert!(restored.close_pane(note));
    restored.detach(bot, 0);
    assert_eq!(restored.tab_of(bot), Some(bot));
    assert_eq!(restored.tab_of(document), Some(17));
    assert_eq!(restored.active, Some(bot));
    restored.tabs.swap(0, 1);
    restored.title_order.swap(0, 1);
    assert_eq!(restored.tabs, [17, bot]);
    let saved = serde_json::to_string(&restored).unwrap();
    let mut reopened = Workspace::restore(Some(&saved), &[(17, "/work".into())], "/work");
    assert_eq!(reopened.tabs, [17, bot]);
    assert_eq!(reopened.active, Some(bot));
    assert_eq!(reopened.tab_of(document), Some(17));
    assert_eq!(reopened.tab_of(note), None);
    reopened.close(bot);
    assert_eq!(reopened.active, Some(document));
    assert!(reopened.app_panes.contains_key(&bot));
}

#[test]
fn reviews_from_two_bots_and_an_unowned_review_have_distinct_pane_ids() {
    let mut workspace = Workspace::default();
    let mut bot_review = |bot: &str| {
        workspace
            .standalone_review_pane("/work".into(), "/copy".into(), Some(bot.into()))
            .unwrap()
    };
    let first = bot_review("first");
    let second = bot_review("second");
    assert_ne!(first, second);
    assert_eq!(bot_review("first"), first);
    let unowned = workspace
        .app_pane(AppPane::Review, "/work".into(), "/copy".into())
        .unwrap();
    assert_ne!(unowned, first);
    assert_ne!(unowned, second);
    assert_eq!(workspace.app_panes[&unowned].review_bot, None);
}

#[test]
fn closed_terminal_descriptor_reserves_its_process_identity() {
    let mut workspace = Workspace::default();
    workspace.app_panes.insert(
        -8,
        AppPaneState {
            content: AppPane::Terminal {
                id: 1200,
                cwd: "/closed".into(),
            },
            project: "/work".into(),
            worktree: "/work".into(),
            review_bot: None,
        },
    );
    let saved = serde_json::to_string(&workspace).unwrap();
    let restored = Workspace::restore(Some(&saved), &[], "/work");
    assert!(restored.tabs.is_empty());
    assert!(restored.app_panes.contains_key(&-8));
    let TitleTab::Terminal { id, .. } = restored.new_terminal_tab("/work".into()) else {
        panic!("new terminal did not receive an identity");
    };
    assert!(id > 1200);
    assert_eq!(restored.fresh_pane_id(), Some(-9));
}

#[test]
fn legacy_scope_only_destinations_do_not_open_tabs() {
    let saved = serde_json::json!({
        "project": "/work",
        "scope_focus": {
            "bot": {"Bot": {"id": "bot-a", "project": "/work", "worktree": "/work"}},
            "terminal": {"terminal": {"cwd": "/work"}},
            "search": "Search"
        }
    })
    .to_string();
    let restored = Workspace::restore(Some(&saved), &[], "/work");
    assert!(restored.tabs.is_empty());
    assert!(restored.title_order.is_empty());
    assert!(restored.scope_focus.is_empty());
}
