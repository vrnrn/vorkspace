//! Native menus dispatch through the same workspace and editor action owners.
use crate::{file_editor as editor, image_lightbox, input, settings::Preferences, shell};
use gpui::{
    Action, AnyWindowHandle, App, Global, KeyBinding, Menu, MenuItem, OsAction, SystemMenuType,
    actions,
};

#[derive(Default)]
struct WorkspaceTarget(Option<AnyWindowHandle>);
impl Global for WorkspaceTarget {}

pub(crate) fn remember(window: &gpui::Window, cx: &mut App) {
    cx.set_global(WorkspaceTarget(Some(window.window_handle())));
}

pub(crate) fn workspace_target(cx: &App) -> Option<AnyWindowHandle> {
    let windows = cx.windows();
    let live = |handle: &AnyWindowHandle| {
        handle.downcast::<shell::Shell>().is_some() && windows.contains(handle)
    };
    cx.active_window()
        .filter(live)
        .or_else(|| {
            cx.try_global::<WorkspaceTarget>()
                .and_then(|target| target.0)
                .filter(live)
        })
        .or_else(|| cx.windows().into_iter().find(live))
}

/// Menus can run while an embedded native view owns first responder or the
/// workspace is hidden. Select one live workspace and
/// dispatch on its root, retaining the normal action and unsaved-file owners.
fn workspace_fallback<A: Action>(cx: &mut App) {
    cx.on_action(|action: &A, cx| {
        let Some(target) = workspace_target(cx) else {
            return;
        };
        let action = action.boxed_clone();
        cx.defer(move |cx| {
            let _ = target.update(cx, |root, window, cx| {
                let Ok(shell) = root.downcast::<shell::Shell>() else {
                    return;
                };
                let focus = shell.read(cx).menu_focus();
                if !window.is_action_available_in(action.as_ref(), &focus) {
                    return;
                }
                cx.activate(true);
                window.activate_window();
                crate::traffic_lights::show(window);
                window.focus(&focus, cx);
                focus.dispatch_action(action.as_ref(), window, cx);
            });
        });
    });
}

actions!(
    application_menu,
    [
        About,
        CheckUpdates,
        Hide,
        HideOthers,
        ShowAll,
        Repository,
        ReportBug,
        RequestFeature
    ]
);

pub(crate) fn clipboard_route(action: crate::status_menu::Action, cx: &mut App) {
    let Some(target) = workspace_target(cx) else {
        return;
    };
    cx.defer(move |cx| {
        let _ = target.update(cx, |root, window, cx| {
            if let Ok(shell) = root.downcast::<shell::Shell>() {
                shell.update(cx, |shell, cx| shell.status_menu_action(action, window, cx));
            }
        });
    });
}

pub(crate) fn init(cx: &mut App) {
    cx.on_action(|_: &crate::clipboard::Toggle, cx| {
        let Some(target) = workspace_target(cx) else {
            return;
        };
        // Preserve the external app that will receive the next Paste.
        cx.defer(move |cx| {
            let _ = target.update(cx, |root, window, cx| {
                if let Ok(shell) = root.downcast::<shell::Shell>() {
                    shell.update(cx, |shell, cx| shell.toggle_clipboard(false, window, cx));
                }
            });
        });
    });
    cx.on_action(|_: &crate::clipboard::Settings, cx| {
        clipboard_route(crate::status_menu::Action::Settings, cx)
    });
    cx.on_action(|_: &crate::clipboard::Composer, cx| {
        clipboard_route(crate::status_menu::Action::QuickComposer, cx)
    });
    macro_rules! workspace_commands {
        ($($action:ty),+ $(,)?) => { $(workspace_fallback::<$action>(cx);)+ };
    }
    workspace_commands!(
        crate::Quit,
        crate::Restart,
        shell::OpenSettings,
        shell::OpenProject,
        shell::GoToFile,
        shell::SearchFiles,
        shell::OpenCommandPalette,
        shell::NewCodex,
        shell::NewTerminal,
        shell::SplitRight,
        shell::SplitDown,
        shell::CloseTab,
        shell::CloseOtherTabs,
        shell::CloseAllTabs,
        shell::PreviousTab,
        shell::NextTab,
        shell::ToggleSidebar,
        shell::ToggleRightPanel,
        shell::FocusSearch,
        shell::ShowInbox,
        shell::ShowGitHub,
        shell::ShowGitLab,
        shell::ShowJira,
        shell::ShowNotes,
        shell::ShowAutomations,
        shell::ToggleTerminal,
        shell::SwitchModel,
        shell::VisitBack,
        shell::VisitForward,
        shell::FocusPaneLeft,
        shell::FocusPaneRight,
        shell::FocusPaneUp,
        shell::FocusPaneDown,
        shell::OpenAppearance,
        image_lightbox::ZoomIn,
        image_lightbox::ZoomOut,
        image_lightbox::ResetZoom,
    );
    cx.on_action(|_: &shell::MinimizeWindow, cx| {
        window_command(cx, |window| window.minimize_window())
    });
    cx.on_action(|_: &shell::ZoomWindow, cx| window_command(cx, |window| window.zoom_window()));
    cx.on_action(|_: &shell::ToggleFullScreen, cx| {
        window_command(cx, |window| window.toggle_fullscreen())
    });
    cx.on_action(|_: &About, _cx| {
        if let Some(main) = objc2::MainThreadMarker::new() {
            objc2_app_kit::NSApplication::sharedApplication(main)
                .orderFrontStandardAboutPanel(None);
        }
    });
    cx.on_action(|_: &CheckUpdates, cx| crate::updater::check(cx));
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &Repository, cx| cx.open_url("https://github.com/vrnrn/vorkspace"));
    cx.on_action(|_: &ReportBug, cx| {
        cx.open_url("https://github.com/vrnrn/vorkspace/issues/new?template=bug_report.yml")
    });
    cx.on_action(|_: &RequestFeature, cx| {
        cx.open_url("https://github.com/vrnrn/vorkspace/issues/new?template=feature_request.yml")
    });
    cx.bind_keys([
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("cmd-alt-h", HideOthers, None),
        KeyBinding::new("cmd-m", shell::MinimizeWindow, None),
        KeyBinding::new("ctrl-cmd-f", shell::ToggleFullScreen, None),
    ]);
}

fn window_command(cx: &mut App, command: impl FnOnce(&mut gpui::Window) + 'static) {
    if let Some(target) = cx.active_window().or_else(|| workspace_target(cx)) {
        cx.defer(move |cx| {
            let _ = target.update(cx, |_, window, _| command(window));
        });
    }
}

pub(crate) fn build(prefs: &Preferences) -> Vec<Menu> {
    vec![
        Menu::new("vorkspace").items([
            MenuItem::action("About vorkspace", About),
            MenuItem::separator(),
            MenuItem::action("Settings…", shell::OpenSettings),
            MenuItem::action("Check for Updates…", CheckUpdates),
            MenuItem::separator(),
            MenuItem::action("Restart vorkspace", crate::Restart),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide vorkspace", Hide),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit vorkspace", crate::Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("Open Project…", shell::OpenProject),
            MenuItem::action("Go to File…", shell::GoToFile),
            MenuItem::action("Find in Files…", shell::SearchFiles),
            MenuItem::action("Command Palette…", shell::OpenCommandPalette),
            MenuItem::separator(),
            MenuItem::action("Save", editor::Save),
            MenuItem::action("Reload File", editor::ReloadFile),
            MenuItem::action("Autosave", editor::ToggleAutosave).checked(prefs.autosave),
            MenuItem::separator(),
            MenuItem::action("New Session", shell::NewCodex),
            MenuItem::action("New Terminal", shell::NewTerminal),
            MenuItem::action("Split Right", shell::SplitRight),
            MenuItem::action("Split Down", shell::SplitDown),
            MenuItem::separator(),
            MenuItem::action("Close Tab", shell::CloseTab),
            MenuItem::action("Close Other Tabs", shell::CloseOtherTabs),
            MenuItem::action("Close All Tabs", shell::CloseAllTabs),
            MenuItem::separator(),
            MenuItem::action("Previous Tab", shell::PreviousTab),
            MenuItem::action("Next Tab", shell::NextTab),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", input::Undo, OsAction::Undo),
            MenuItem::os_action("Redo", input::Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", input::Cut, OsAction::Cut),
            MenuItem::os_action("Copy", input::Copy, OsAction::Copy),
            MenuItem::os_action("Paste", input::Paste, OsAction::Paste),
            MenuItem::os_action("Select All", input::SelectAll, OsAction::SelectAll),
            MenuItem::separator(),
            MenuItem::action("Find", editor::Find),
            MenuItem::action("Replace", editor::Replace),
            MenuItem::action("Format Document", editor::Format),
            MenuItem::action("Word Wrap", editor::ToggleWordWrap).checked(prefs.file_word_wrap),
            MenuItem::separator(),
            MenuItem::action("Fold", input::FoldCode),
            MenuItem::action("Unfold", input::UnfoldCode),
            MenuItem::action("Fold All", input::FoldAll),
            MenuItem::action("Unfold All", input::UnfoldAll),
        ]),
        Menu::new("View").items([
            MenuItem::action("Toggle Project Rail", shell::ToggleSidebar),
            MenuItem::action("Toggle Workspace Sidebar", shell::ToggleRightPanel),
            MenuItem::separator(),
            MenuItem::action("Search", shell::FocusSearch),
            MenuItem::action("GitHub", shell::ShowGitHub),
            MenuItem::action("GitLab", shell::ShowGitLab),
            MenuItem::action("Jira", shell::ShowJira),
            MenuItem::action("Notes", shell::ShowNotes),
            MenuItem::action("Automations", shell::ShowAutomations),
            MenuItem::action("Toggle Terminal", shell::ToggleTerminal),
            MenuItem::action("Switch Model…", shell::SwitchModel),
            MenuItem::separator(),
            MenuItem::action("Back", shell::VisitBack),
            MenuItem::action("Forward", shell::VisitForward),
            MenuItem::action("Focus Left", shell::FocusPaneLeft),
            MenuItem::action("Focus Right", shell::FocusPaneRight),
            MenuItem::action("Focus Up", shell::FocusPaneUp),
            MenuItem::action("Focus Down", shell::FocusPaneDown),
            MenuItem::separator(),
            MenuItem::action("Zoom In", image_lightbox::ZoomIn),
            MenuItem::action("Zoom Out", image_lightbox::ZoomOut),
            MenuItem::action("Reset Zoom", image_lightbox::ResetZoom),
            MenuItem::action("Appearance…", shell::OpenAppearance),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", shell::MinimizeWindow),
            MenuItem::action("Zoom", shell::ZoomWindow),
            MenuItem::action("Toggle Full Screen", shell::ToggleFullScreen),
        ]),
        Menu::new("Help").items([
            MenuItem::action("View on GitHub", Repository),
            MenuItem::action("Report a Bug…", ReportBug),
            MenuItem::action("Request a Feature…", RequestFeature),
        ]),
    ]
}
