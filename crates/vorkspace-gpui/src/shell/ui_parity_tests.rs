//! Opt-in pixel parity. Renders the main surfaces offscreen so a refactor can be
//! compared byte for byte against the captures of the commit before it.
use super::*;
use crate::{right_panel::RightTab, settings::Section};
use vorkspace_core::store::Store;

const PROJECT: &str = "/tmp/vorkspace-ui-parity";
type Open = fn(&mut Shell, &mut Window, &mut Context<Shell>);

fn git(args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(PROJECT)
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
        .status()
        .unwrap();
    assert!(status.success());
}

/// A fixed repository: the right panel only opens for a checkout, and its
/// contents must not vary between the two runs being compared.
fn project() {
    let _ = std::fs::remove_dir_all(PROJECT);
    std::fs::create_dir_all(format!("{PROJECT}/src")).unwrap();
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.name", "Parity"]);
    git(&["config", "user.email", "parity@example.invalid"]);
    std::fs::write(format!("{PROJECT}/README.md"), "# Parity\n").unwrap();
    std::fs::write(format!("{PROJECT}/src/main.rs"), "fn main() {}\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "Add the parity fixture"]);
    std::fs::write(
        format!("{PROJECT}/src/main.rs"),
        "fn main() {\n    println!(\"changed\");\n}\n",
    )
    .unwrap();
}

fn right_tab(shell: &mut Shell, tab: RightTab, window: &mut Window, cx: &mut Context<Shell>) {
    shell.set_right_panel_open(true, window, cx);
    if let Some(panel) = &shell.right_panel {
        panel.update(cx, |panel, cx| panel.select(tab, cx));
    }
}

const SURFACES: &[(&str, Open)] = &[
    ("chat", |_, _, _| {}),
    ("explorer", |shell, window, cx| {
        right_tab(shell, RightTab::Files, window, cx)
    }),
    ("changes", |shell, window, cx| {
        right_tab(shell, RightTab::Changes, window, cx)
    }),
    ("graph", |shell, window, cx| {
        right_tab(shell, RightTab::Graph, window, cx)
    }),
    ("notes", |shell, window, cx| shell.open_notes(window, cx)),
    ("todos", |shell, window, cx| shell.open_todos(window, cx)),
    ("inbox", |shell, window, cx| shell.open_inbox(window, cx)),
    ("automations", |shell, window, cx| {
        shell.open_automations(window, cx)
    }),
    ("search", |shell, window, cx| {
        shell.open_app_search(window, cx)
    }),
    ("settings-general", |shell, window, cx| {
        shell.open_settings(Section::General, window, cx)
    }),
    ("settings-connections", |shell, window, cx| {
        shell.open_settings(Section::Connections, window, cx)
    }),
    ("settings-appearance", |shell, window, cx| {
        shell.open_settings(Section::Appearance, window, cx)
    }),
    ("settings-keybindings", |shell, window, cx| {
        shell.open_settings(Section::Keybindings, window, cx)
    }),
    ("settings-chat", |shell, window, cx| {
        shell.open_settings(Section::Chat, window, cx)
    }),
    ("settings-providers", |shell, window, cx| {
        shell.open_settings(Section::Providers, window, cx)
    }),
    ("settings-bots", |shell, window, cx| {
        shell.open_settings(Section::Bots, window, cx)
    }),
    ("settings-archive", |shell, window, cx| {
        shell.open_settings(Section::Archive, window, cx)
    }),
];

#[test]
#[ignore = "Requires Metal and VORKSPACE_UI_PARITY_DIR"]
fn ui_parity_capture() {
    let output = std::path::PathBuf::from(std::env::var_os("VORKSPACE_UI_PARITY_DIR").unwrap());
    assert!(output.is_absolute() && output.is_dir());
    project();
    let store = Arc::new(Mutex::new(
        Store::open(std::path::Path::new(":memory:")).unwrap(),
    ));
    let session = {
        let mut store = locked(&store).unwrap();
        let mut session = store.create("codex", PROJECT).unwrap();
        store
            .write_rows(
                session.id,
                0,
                &[
                    vorkspace_core::transcript::Row {
                        role: "user".into(),
                        text: "Summarize the change and show the entry point.".into(),
                        first: true,
                        sent_at: None,
                    },
                    vorkspace_core::transcript::Row {
                        role: "assistant".into(),
                        text: "The entry point now **prints** a line:\n\n```rust\nfn main() {\n    println!(\"changed\");\n}\n```\n\n- One file changed\n- No tests affected".into(),
                        first: true,
                        sent_at: None,
                    },
                ],
            )
            .unwrap();
        session.has_messages = true;
        session
    };
    let mut cx = gpui::HeadlessAppContext::with_platform(
        Arc::new(gpui_apple::AppleTextSystem::new()),
        Arc::new(crate::assets::Assets),
        || {
            Ok(Some(Box::new(
                gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
            )))
        },
    );
    crate::clock::use_test_clock(&cx);
    for theme in ["dark", "light"] {
        for (name, open) in SURFACES {
            let mut state = Workspace::default();
            state.preferences.theme = theme.into();
            state.add_project(PROJECT);
            state.project = PROJECT.into();
            state.open(session.id);
            let saved = serde_json::to_string(&state).unwrap();
            let store = store.clone();
            let summaries = vec![session.clone()];
            let window = cx
                .open_window(gpui::size(px(1400.), px(900.)), |_, cx| {
                    cx.new(|cx| Shell::new(store, summaries, PROJECT.into(), Some(saved), cx))
                })
                .unwrap();
            cx.run_until_parked();
            // Git status arrives from a worker thread that parking does not await.
            std::thread::sleep(std::time::Duration::from_millis(400));
            cx.run_until_parked();
            window.update(&mut cx, open).unwrap();
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(400));
            crate::clock::settle_headless_animations(window, &mut cx);
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(output.join(format!("{theme}-{name}.png")))
                .unwrap();
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
        }
    }
}
