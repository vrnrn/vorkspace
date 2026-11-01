
use super::*;
use crate::settings::{Preferences, Section, SettingsContext};
use gpui::{
    Bounds, TestAppContext, VisualTestContext, WindowBounds, WindowHandle, WindowOptions, point,
    size,
};
use std::{cell::RefCell, rc::Rc};

fn window(cx: &mut TestAppContext) -> WindowHandle<SettingsView> {
    cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1100.), px(1000.)),
                ))),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|cx| {
                    let mut view = SettingsView::new(
                        Section::Connectors,
                        Preferences::default(),
                        SettingsContext::default(),
                        cx,
                    );
                    view.discovery_task = None;
                    view.discovery_loading = false;
                    view
                })
            },
        )
        .unwrap()
    })
}
#[gpui::test]
fn inbox_settings_offer_only_supported_sources_and_emit_independent_enable_choice(
    cx: &mut TestAppContext,
) {
    let window = window(cx);
    let picked = Rc::new(RefCell::new(Vec::new()));
    let output = picked.clone();
    let _subscription = window
        .update(cx, |_, _, cx| {
            cx.subscribe(&cx.entity(), move |_, _, event: &SettingsEvent, _| {
                if let SettingsEvent::InboxConnectorEnabled(source, enabled) = event {
                    output.borrow_mut().push((source.clone(), *enabled));
                }
            })
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    for selector in [
        "settings-toggle-inbox-connector-github",
        "settings-toggle-inbox-connector-gitlab",
        "settings-toggle-inbox-connector-jira",
    ] {
        assert!(visual.debug_bounds(selector).is_some(), "{selector}");
    }
    for selector in [
        "settings-toggle-inbox-connector-linear",
        "settings-toggle-inbox-connector-azuredevops",
    ] {
        assert!(visual.debug_bounds(selector).is_none());
    }
    let github = visual
        .debug_bounds("settings-toggle-inbox-connector-github")
        .unwrap();
    visual.simulate_click(github.center(), Default::default());
    visual.run_until_parked();
    assert_eq!(*picked.borrow(), vec![("github".into(), false)]);
    window
        .update(visual, |view, _, cx| {
            let mut context = view.ctx.clone();
            context.inbox_connectors.set("github", false).unwrap();
            view.set_context(context, cx);
        })
        .unwrap();
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("settings-toggle-inbox-connector-github")
            .is_some()
    );
    assert!(
        visual
            .debug_bounds("settings-select-gitlab-default-bot")
            .is_some()
    );
}

#[gpui::test]
fn jql_validation_rejects_stale_edits_mode_changes_and_errors_before_publishing(
    cx: &mut TestAppContext,
) {
    let window = window(cx);
    let applied = Rc::new(RefCell::new(Vec::new()));
    let output = applied.clone();
    let _subscription = window
        .update(cx, |view, _, cx| {
            let subscription = cx.subscribe(&cx.entity(), move |_, _, event: &SettingsEvent, _| {
                if let SettingsEvent::JiraFilter(filter) = event {
                    output.borrow_mut().push(filter.clone());
                }
            });
            view.initialize_jira_query(cx);
            view.inbox.mode = "jql".into();
            view.ctx
                .inbox_connections
                .insert("jira".into(), (true, "https://team.atlassian.net".into()));
            subscription
        })
        .unwrap();
    for (query, current, mode, result) in [
        ("filter = 1", "filter = 2", "jql", Ok(())),
        ("filter = 1", "filter = 1", "project", Ok(())),
        (
            "invalid query",
            "invalid query",
            "jql",
            Err("Invalid JQL".into()),
        ),
    ] {
        window
            .update(cx, |view, _, cx| {
                view.inbox
                    .query
                    .as_ref()
                    .unwrap()
                    .update(cx, |input, cx| input.set_text(current.into(), cx));
                view.inbox.mode = mode.into();
                view.inbox.revision += 1;
                view.inbox.applying = true;
                view.inbox.pending = Some(view.inbox.revision);
                view.finish_jira_jql(
                    view.inbox.revision,
                    query.into(),
                    view.ctx.inbox_connections.get("jira").cloned(),
                    result,
                    cx,
                );
                assert_eq!(view.ctx.jira_filter.mode, "project");
                assert!(!view.inbox.applying);
            })
            .unwrap();
        cx.run_until_parked();
        assert!(applied.borrow().is_empty());
    }
    window
        .update(cx, |view, _, cx| {
            view.inbox
                .query
                .as_ref()
                .unwrap()
                .update(cx, |input, cx| input.set_text("filter = 12345".into(), cx));
            view.inbox.revision += 1;
            view.inbox.mode = "jql".into();
            view.inbox.applying = true;
            view.inbox.pending = Some(view.inbox.revision);
            let revision = view.inbox.revision;
            view.finish_jira_jql(
                revision - 1,
                "filter = 12345".into(),
                view.ctx.inbox_connections.get("jira").cloned(),
                Ok(()),
                cx,
            );
            assert!(
                view.inbox.applying,
                "older request cannot clear a newer request's activity"
            );
            view.finish_jira_jql(
                revision,
                "filter = 12345".into(),
                view.ctx.inbox_connections.get("jira").cloned(),
                Ok(()),
                cx,
            );
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        *applied.borrow(),
        vec![JiraFilter {
            mode: "jql".into(),
            jql: "filter = 12345".into()
        }]
    );
    window
        .update(cx, |view, _, cx| {
            view.inbox.applying = true;
            view.inbox.pending = Some(view.inbox.revision);
            view.ctx.inbox_connectors.set("jira", false).unwrap();
            view.finish_jira_jql(
                view.inbox.revision,
                "filter = 12345".into(),
                view.ctx.inbox_connections.get("jira").cloned(),
                Ok(()),
                cx,
            );
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        applied.borrow().len(),
        1,
        "a disabled connector cannot accept a late validation"
    );
}
