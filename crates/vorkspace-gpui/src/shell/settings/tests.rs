
use super::*;
use crate::shell::tests::window;
use gpui::TestAppContext;

#[gpui::test]
fn project_archive_leaves_the_selected_project_and_refuses_running_work(cx: &mut TestAppContext) {
    let (window, a, _) = window(cx);
    window
        .update(cx, |shell, w, cx| {
            // Running work in a project refuses its archive.
            shell.sessions[&a].update(cx, |view, _| view.busy = true);
            let busy_project = shell.session_metadata(a, cx).summary.cwd.clone();
            shell.archive_project(&busy_project, w, cx);
            assert!(
                shell
                    .error
                    .as_deref()
                    .is_some_and(|e| e.contains("running conversations"))
            );
            assert_ne!(
                shell.workspace.project_archive.get(&busy_project),
                Some(&true)
            );
            shell.sessions[&a].update(cx, |view, _| view.busy = false);
            // The selected project can be archived: another one is selected.
            shell.workspace.add_project("/work/archive");
            shell.workspace.project = "/work/archive".into();
            shell.archive_project("/work/archive", w, cx);
            assert_eq!(
                shell.workspace.project_archive.get("/work/archive"),
                Some(&true)
            );
            assert!(!shell.workspace.projects.contains(&"/work/archive".into()));
            assert_ne!(shell.workspace.project, "/work/archive");
        })
        .unwrap();
    window
        .update(cx, |shell, w, cx| {
            shell.open_settings(Section::Archive, w, cx)
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let restore = visual
        .debug_bounds("restore-project-/work/archive")
        .expect("restore control");
    visual.simulate_click(restore.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            assert_eq!(
                shell.workspace.project_archive.get("/work/archive"),
                Some(&false)
            );
            assert!(shell.workspace.projects.contains(&"/work/archive".into()));
            let saved = locked(&shell.store).unwrap().ui_state().unwrap().unwrap();
            let saved: serde_json::Value = serde_json::from_str(&saved).unwrap();
            assert_eq!(saved["project_archive"]["/work/archive"], false);
        })
        .unwrap();
}

#[gpui::test]
fn settings_skill_form_opens_and_cancels_without_leaving_settings(cx: &mut TestAppContext) {
    cx.update(super::super::bind_keys);
    let (window, _, _) = window(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.open_settings(Section::Skills, w, cx)
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let add = visual.debug_bounds("settings-add-skill").unwrap();
    visual.simulate_click(add.center(), Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("settings-skill-form").is_some());
    assert!(visual.debug_bounds("settings-management-dialog").is_none());
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(visual.debug_bounds("settings-skill-form").is_none());
    window
        .update(visual, |shell, _, _| assert!(shell.settings.is_some()))
        .unwrap();
}

#[gpui::test]
fn escape_closes_the_inner_settings_dialog_first(cx: &mut TestAppContext) {
    cx.update(super::super::bind_keys);
    let (window, _, _) = window(cx);
    window
        .update(cx, |shell, w, cx| shell.open_settings(Section::Mcp, w, cx))
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let add = visual.debug_bounds("settings-add-mcp").unwrap();
    visual.simulate_click(add.center(), Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("settings-management-dialog").is_some());
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| assert!(shell.settings.is_some()))
        .unwrap();
    assert!(visual.debug_bounds("settings-management-dialog").is_none());
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| assert!(shell.settings.is_none()))
        .unwrap();
}
#[gpui::test]
fn settings_replace_native_rail_and_tabs_and_return_to_the_same_session(cx: &mut TestAppContext) {
    cx.update(super::super::bind_keys);
    let (window, _, active) = window(cx);
    window
        .update(cx, |shell, w, cx| shell.focus_active(w, cx))
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_keystrokes("cmd-,");
    visual.run_until_parked();
    let navigation = visual.debug_bounds("settings-nav").unwrap();
    let content = visual.debug_bounds("settings-view").unwrap();
    let header = visual.debug_bounds("settings-header").unwrap();
    let page = visual.debug_bounds("settings-page-header").unwrap();
    assert!(
        header.bottom() <= px(40.),
        "breadcrumb and search live in the title bar"
    );
    assert!(
        navigation.right() <= content.left(),
        "settings navigation stays beside the page: navigation={navigation:?}, content={content:?}"
    );
    assert!(
        (page.left()
            - content.left()
            - (content.size.width - px(1024.)).max(px(0.)) / 2.
            - px(32.))
        .abs()
            < px(1.),
        "the centered page keeps its 32px padding: page={page:?}, content={content:?}"
    );
    assert!(visual.debug_bounds("settings-toggle-sounds").is_some());
    assert!(visual.debug_bounds("sidebar-content").is_none());
    for (section, selector) in Section::ALL.into_iter().zip([
        "settings-nav-general",
        "settings-nav-connections",
        "settings-nav-appearance",
        "settings-nav-keybindings",
        "settings-nav-chat",
        "settings-nav-providers",
        "settings-nav-mcp",
        "settings-nav-skills",
        "settings-nav-bots",
        "settings-nav-connectors",
        "settings-nav-archive",
        "settings-nav-worktrees",
        "settings-nav-vox",
    ]) {
        let bounds = visual.debug_bounds(selector).unwrap();
        visual.simulate_click(bounds.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, cx| {
                assert_eq!(shell.settings.as_ref().unwrap().read(cx).section, section)
            })
            .unwrap();
        assert!(
            visual.debug_bounds("settings-page-header").is_some(),
            "page {} renders",
            section.label()
        );
    }
    let back = visual.debug_bounds("settings-nav-back").unwrap();
    visual.simulate_click(back.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            assert!(shell.settings.is_none());
            assert_eq!(shell.workspace.active, Some(active));
            assert_eq!(shell.workspace.tabs.len(), 2);
        })
        .unwrap();
    assert!(visual.debug_bounds("sidebar-content").is_some());
}
#[gpui::test]
fn native_setting_edits_persist_and_theme_switches_live(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.open_settings(Section::Appearance, w, cx)
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("settings-toggle-inset-layout")
            .is_none()
    );
    let light = visual.debug_bounds("settings-theme-light").unwrap();
    visual.simulate_click(light.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            assert_eq!(shell.workspace.preferences.theme, "light");
            let saved = shell.latest_state.lock().unwrap().clone();
            let restored: Workspace = serde_json::from_str(&saved).unwrap();
            assert_eq!(restored.preferences.theme, "light");
            assert!(
                content(1.).l < 0.3,
                "the native text ink switched to the light palette"
            );
        })
        .unwrap();
    let glass = visual
        .debug_bounds("settings-toggle-main-pane-glass")
        .unwrap();
    assert!(glass.size.width > px(0.));
    let dark = visual.debug_bounds("settings-theme-dark").unwrap();
    visual.simulate_click(dark.center(), Default::default());
    visual.run_until_parked();
}

#[gpui::test]
fn shortcut_recording_rebinds_native_settings_and_reset_restores_it(cx: &mut TestAppContext) {
    cx.update(crate::input::bind);
    cx.update(super::super::bind_keys);
    let (window, _, _) = window(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.open_settings(Section::Keybindings, w, cx)
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let recorder = visual.debug_bounds("shortcut-App: Settings").unwrap();
    visual.simulate_click(recorder.center(), Default::default());
    visual.simulate_keystrokes("cmd-k");
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            assert!(
                !shell
                    .workspace
                    .preferences
                    .keybindings
                    .contains_key("App: Settings"),
                "a default shortcut cannot be claimed twice"
            )
        })
        .unwrap();
    visual.simulate_keystrokes("cmd-alt-y");
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            assert!(
                shell.workspace.preferences.keybindings["App: Settings"]
                    .as_ref()
                    .unwrap()
                    .ends_with('y')
            )
        })
        .unwrap();
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| assert!(shell.settings.is_none()))
        .unwrap();
    visual.simulate_keystrokes("cmd-,");
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            assert!(shell.settings.is_none(), "old chord is removed")
        })
        .unwrap();
    visual.simulate_keystrokes("cmd-alt-y");
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            assert!(
                shell.settings.is_some(),
                "the newly recorded chord dispatches the native action"
            )
        })
        .unwrap();
    window
        .update(visual, |shell, _, cx| {
            shell.select_settings(Section::Keybindings, cx)
        })
        .unwrap();
    visual.run_until_parked();
    let reset = visual.debug_bounds("reset-shortcut-App: Settings").unwrap();
    visual.simulate_click(reset.center(), Default::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-,");
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            assert!(
                shell.settings.is_some(),
                "reset restores the native default shortcut"
            )
        })
        .unwrap();
}
