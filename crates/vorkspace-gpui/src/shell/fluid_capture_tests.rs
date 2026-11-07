//! Opt-in captures of the fluidither layer: the shell with several projects and
//! sessions, the tool-heavy chat fixture, and pointer hovers that show the
//! dithered states. Disposable in-memory data only.
use super::*;
use vorkspace_core::store::Store;

const PROJECT: &str = "/tmp/vorkspace-fluid-capture";

/// What each capture shows: a name, the session index to open (`None` opens
/// Settings → Appearance), and a hover point in logical points for 1400×900.
const SHOTS: &[(&str, Option<usize>, (f32, f32))] = &[
    ("rest", Some(0), (1390., 890.)),
    ("hover-session", Some(0), (420., 165.)),
    ("hover-nav", Some(0), (90., 192.)),
    ("hover-tab", Some(0), (720., 20.)),
    ("visuals", Some(5), (1390., 890.)),
    ("mermaid", Some(6), (1390., 890.)),
    ("empty-session", Some(7), (1390., 890.)),
    ("settings", None, (1390., 890.)),
    ("notice", Some(4), (1390., 890.)),
    ("live", Some(2), (1390., 890.)),
    ("menu", Some(0), (1390., 890.)),
    ("glide", Some(1), (1390., 890.)),
    ("tasks", Some(3), (1390., 890.)),
    ("work", Some(0), (1390., 890.)),
    ("automation-editor", Some(0), (1390., 890.)),
    ("approval-inline", Some(0), (1390., 890.)),
    ("tooltip", Some(0), (551., 271.)),
    ("explorer", Some(0), (1390., 890.)),
    ("changes", Some(0), (1390., 890.)),
    ("notes", Some(0), (1390., 890.)),
    ("automations", Some(0), (1390., 890.)),
    ("todos", Some(0), (1390., 890.)),
    ("quick-open", Some(0), (1390., 890.)),
    ("palette", Some(0), (1390., 890.)),
    ("settings-general", None, (1390., 890.)),
    ("settings-keybindings", None, (1390., 890.)),
    ("settings-providers", None, (1390., 890.)),
    ("settings-chat", None, (1390., 890.)),
    ("settings-connections", None, (1390., 890.)),
    ("settings-mcp", None, (1390., 890.)),
    ("settings-skills", None, (1390., 890.)),
    ("settings-bots", None, (1390., 890.)),
    ("settings-vox", None, (1390., 890.)),
    ("settings-plugins", None, (1390., 890.)),
    ("settings-archive", None, (1390., 890.)),
    ("settings-worktrees", None, (1390., 890.)),
    ("settings-notes", None, (1390., 890.)),
    ("settings-todos", None, (1390., 890.)),
    ("settings-github", None, (1390., 890.)),
    ("diff", Some(0), (1390., 890.)),
    ("file", Some(0), (1390., 890.)),
    ("github", Some(0), (1390., 890.)),
    ("search", Some(0), (1390., 890.)),
    ("typed", Some(0), (1390., 890.)),
    ("project-menu", Some(0), (1390., 890.)),
    ("bot", Some(0), (1390., 890.)),
    ("bot-new", Some(0), (1390., 890.)),
    ("bot-soul", Some(0), (1390., 890.)),
    ("bot-memory", Some(0), (1390., 890.)),
    ("bot-habits", Some(0), (1390., 890.)),
    ("rest-narrow", Some(0), (850., 610.)),
    ("settings-narrow", None, (850., 610.)),
    ("project-appearance-narrow", Some(0), (850., 610.)),
    ("bot-narrow", Some(0), (850., 610.)),
    ("bot-soul-narrow", Some(0), (850., 610.)),
    ("palette-narrow", Some(0), (850., 610.)),
    ("project-appearance", Some(0), (1390., 890.)),
    ("project-background", Some(0), (1390., 890.)),
    ("project-codebase", Some(0), (1390., 890.)),
    ("project-notifications", Some(0), (1390., 890.)),
];

/// A small committed project with uncommitted edits, so Explorer and Changes
/// have something to show.
fn seed_project() {
    let root = std::path::Path::new(PROJECT);
    let write = |path: &str, text: &str| {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    if !root.join(".git").exists() {
        write(
            "Cargo.toml",
            "[package]\nname = \"glide\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        );
        write("README.md", "# Glide\n\nA spring that lands.\n");
        write("src/main.rs", "fn main() {\n    println!(\"glide\");\n}\n");
        write(
            "src/spring.rs",
            "pub struct Spring {\n    pub position: f64,\n    pub velocity: f64,\n}\n",
        );
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .env("GIT_AUTHOR_NAME", "Capture")
                .env("GIT_AUTHOR_EMAIL", "capture@example.invalid")
                .env("GIT_COMMITTER_NAME", "Capture")
                .env("GIT_COMMITTER_EMAIL", "capture@example.invalid")
                .output()
                .unwrap();
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "Start"]);
    }
    write(
        "src/spring.rs",
        "pub struct Spring {\n    pub position: f64,\n    pub velocity: f64,\n    pub target: f64,\n}\n\nimpl Spring {\n    pub fn landed(&self) -> bool {\n        (self.position - self.target).abs() < 0.01\n    }\n}\n",
    );
    write("src/glide.rs", "pub fn glide() {}\n");
    write("README.md", "# Glide\n");
}

fn rows(pairs: &[(&str, &str)]) -> Vec<vorkspace_core::transcript::Row> {
    pairs
        .iter()
        .map(|(role, text)| vorkspace_core::transcript::Row {
            role: (*role).into(),
            text: (*text).into(),
            first: true,
            sent_at: None,
        })
        .collect()
}

#[test]
#[ignore = "Requires Metal and VORKSPACE_FLUID_CAPTURE_DIR"]
fn fluid_capture() {
    let output = std::path::PathBuf::from(std::env::var_os("VORKSPACE_FLUID_CAPTURE_DIR").unwrap());
    assert!(output.is_absolute() && output.is_dir());
    std::fs::create_dir_all(PROJECT).unwrap();
    seed_project();
    let store = Arc::new(Mutex::new(
        Store::open(std::path::Path::new(":memory:")).unwrap(),
    ));
    {
        // A populated board, so To-dos shows its columns in use.
        use vorkspace_core::todos::{TodoBoard, TodoStatus};
        let store = locked(&store).unwrap();
        let board = TodoBoard::Personal;
        for (title, status) in [
            ("Port the glide to segmented controls", TodoStatus::Done),
            ("Square the switch tracks", TodoStatus::Done),
            ("Theme every status color", TodoStatus::InReview),
            ("Capture narrow windows", TodoStatus::InProgress),
            ("Audit the Codebase view", TodoStatus::Todo),
            ("Check approval toasts in the shell", TodoStatus::Todo),
        ] {
            store
                .add_todo_with_details(
                    &board,
                    title,
                    status,
                    "Verified in the offscreen captures.",
                    &[],
                    &[],
                )
                .unwrap();
        }
    }
    {
        // Saved notes, so Notes shows its library and an open note.
        let store = locked(&store).unwrap();
        for (id, title, body) in [
            (
                "note-1-0000000000000001",
                "Glide timing",
                "# Glide timing\n\nThe thumb springs on the **GLIDE** curve and lands in 0.18 s.\n\n- Probes record bounds in prepaint\n- The thumb paints after every prepaint\n- A stall over 0.25 s lands at once\n",
            ),
            (
                "note-1-0000000000000002",
                "Status colors",
                "Never write dark-theme literals. Use `danger()`, `warning()` and `success()`.",
            ),
            (
                "note-1-0000000000000003",
                "Release checklist",
                "1. Capture both themes\n2. Run the full suite\n3. Push the branch",
            ),
        ] {
            store
                .upsert_note(&vorkspace_core::notes::NoteUpsert {
                    id: id.into(),
                    title: title.into(),
                    body: body.into(),
                    source_cwd: Some(PROJECT.into()),
                    ..Default::default()
                })
                .unwrap();
        }
    }
    let sessions: Vec<_> = {
        let mut store = locked(&store).unwrap();
        let titles = [
            "Review regression tests",
            "Dither hover states",
            "Spring the sidebar thumb",
            "Obel Mono in code blocks",
            "Fix scroll anchor on resize",
            "Chart the frame budget",
            "Diagram the glide",
            "New session",
        ];
        titles
            .iter()
            .enumerate()
            .map(|(index, title)| {
                let mut session = store.create("codex", PROJECT).unwrap();
                let rows = if index == 0 {
                    let mut rows = crate::fixtures::chat().rows;
                    rows.push(vorkspace_core::transcript::Row {
                        role: "user".into(),
                        text: "Show the spring step.".into(),
                        first: true,
                        sent_at: None,
                    });
                    rows.push(vorkspace_core::transcript::Row {
                        role: "assistant".into(),
                        text: "`Spring::step` advances by the exact solution, so the path ignores frame rate:\n\n```rust\npub fn step(&mut self, dt: f64) -> bool {\n    let (position, velocity) = step_spring(&self.config, self.position, self.velocity, self.target, dt);\n    self.position = position;\n    self.velocity = velocity;\n    self.landed()\n}\n```\n\n- Retargeting keeps velocity\n- `GLIDE` lands in 0.18 s".into(),
                        first: true,
                        sent_at: None,
                    });
                    rows
                } else if index == 5 {
                    // Only the charted dashboard, so its chart is in view.
                    let mut transcript = vorkspace_core::transcript::Transcript::default();
                    transcript.begin("user");
                    transcript.append("Chart frame time per build.");
                    transcript.begin("visual");
                    transcript.append(
                        &serde_json::to_string(&vorkspace_core::visual::VisualDocument {
                            id: "example-dashboard".into(),
                            title: "Throughput explorer".into(),
                            source: include_str!("../../fixtures/visuals/visual-dashboard.js")
                                .into(),
                        })
                        .unwrap(),
                    );
                    transcript.rows
                } else if index == 6 {
                    rows(&[
                        ("user", "Diagram how the thumb glides."),
                        (
                            "assistant",
                            "```mermaid\nflowchart LR\n  A[Click a tab] --> B{Measured?}\n  B -- yes --> C([Spring to target])\n  B -- no --> D[Item paints its own fill]\n  C --> E[Land and stop frames]\n```",
                        ),
                    ])
                } else if index == 3 {
                    let mut transcript = vorkspace_core::transcript::Transcript::default();
                    transcript.begin("user");
                    transcript.append("Plan the Obel Mono switch.");
                    transcript.begin("tool");
                    transcript.rows.last_mut().unwrap().text =
                        serde_json::to_string(&vorkspace_core::tool::ToolCall {
                            id: "plan".into(),
                            name: "TodoWrite".into(),
                            input: r#"{"todos":[{"content":"Load Obel Mono at startup","status":"completed"},{"content":"Switch code blocks and readouts","status":"completed"},{"content":"Keep the terminal on the fixed font","status":"in_progress"},{"content":"Drop the Menlo fallback","status":"cancelled"},{"content":"Capture both themes","status":"pending"}]}"#.into(),
                            output: String::new(),
                            state: vorkspace_core::tool::ToolState::Completed,
                            panel: None,
                            display: None,
                        })
                        .unwrap();
                    transcript.begin("assistant");
                    transcript.append("Two of four done; the terminal keeps Menlo.");
                    transcript.rows
                } else if index == 7 {
                    Vec::new()
                } else {
                    vec![
                        vorkspace_core::transcript::Row {
                            role: "user".into(),
                            text: (*title).into(),
                            first: true,
                            sent_at: None,
                        },
                        vorkspace_core::transcript::Row {
                            role: "assistant".into(),
                            text: "Done. `cargo nextest` passes.\n\n---\n\nThe anchor now holds through a resize.".into(),
                            first: true,
                            sent_at: None,
                        },
                    ]
                };
                if !rows.is_empty() {
                    store.write_rows(session.id, 0, &rows).unwrap();
                    store.title(session.id, title).unwrap();
                    session.title = (*title).into();
                    session.has_messages = true;
                }
                session
            })
            .collect()
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
    vorkspace_ui::clock::use_test_clock(&cx);
    // A comma-separated subset of shot names, to iterate on one surface.
    let only = std::env::var("VORKSPACE_FLUID_CAPTURE_ONLY").ok();
    // Screens that keep asking for frames once settled: an idle app should
    // draw nothing until something changes.
    let mut busy: Vec<String> = Vec::new();
    for theme in ["dark", "light"] {
        for (name, open, at) in SHOTS {
            if only
                .as_deref()
                .is_some_and(|only| !only.split(',').any(|o| o == *name))
            {
                continue;
            }
            // A "-narrow" shot is its base scene in a small window.
            let file = *name;
            let narrow = file.ends_with("-narrow");
            let base = file.trim_end_matches("-narrow");
            let name = &base;
            let mut state = Workspace::default();
            state.preferences.theme = theme.into();
            state.add_project(PROJECT);
            state.project = PROJECT.into();
            if *name == "glide" {
                // Two tabs, so the title strip's thumb has somewhere to go.
                state.open(sessions[0].id);
            }
            state.open(sessions[open.unwrap_or(0)].id);
            let saved = serde_json::to_string(&state).unwrap();
            let store = store.clone();
            let summaries = sessions.clone();
            let window = cx
                .open_window(
                    if narrow {
                        gpui::size(px(860.), px(620.))
                    } else {
                        gpui::size(px(1400.), px(900.))
                    },
                    |_, cx| {
                        cx.new(|cx| Shell::new(store, summaries, PROJECT.into(), Some(saved), cx))
                    },
                )
                .unwrap();
            cx.run_until_parked();
            if *name == "live" {
                let id = sessions[2].id;
                window
                    .update(&mut cx, |shell, _, cx| {
                        let transcript = shell.sessions[&id].read(cx).transcript.clone();
                        transcript.update(cx, |view, cx| {
                            view.streaming(true, cx);
                            view.patch(
                                2,
                                rows(&[
                                    ("user", "Keep the thumb under the tab"),
                                    ("assistant", ""),
                                ]),
                                cx,
                            );
                        });
                    })
                    .unwrap();
                cx.run_until_parked();
            }
            if *name == "work" {
                let id = sessions[0].id;
                window
                    .update(&mut cx, |shell, _, cx| {
                        let transcript = shell.sessions[&id].read(cx).transcript.clone();
                        transcript.update(cx, |view, cx| {
                            assert!(view.open_first_phase(cx), "a run of steps");
                        });
                    })
                    .unwrap();
                cx.run_until_parked();
            }
            if *name == "tasks" {
                let id = sessions[3].id;
                window
                    .update(&mut cx, |shell, _, cx| {
                        let transcript = shell.sessions[&id].read(cx).transcript.clone();
                        transcript.update(cx, |view, cx| {
                            // Open the finished turn's work, where the plan sits.
                            view.open_work(1, cx);
                        });
                    })
                    .unwrap();
                cx.run_until_parked();
            }
            if *name == "menu" {
                let id = sessions[0].id;
                window
                    .update(&mut cx, |shell, w, cx| {
                        shell.open_menu(
                            MenuTarget::Session(id),
                            crate::popover::Anchor::Point(gpui::point(px(470.), px(200.))),
                            w,
                            cx,
                        )
                    })
                    .unwrap();
                cx.run_until_parked();
            }
            if *name == "notice" {
                window
                    .update(&mut cx, |shell, _, cx| {
                        shell.error = Some("Codex exited with status 1: rate limited.".into());
                        cx.notify();
                    })
                    .unwrap();
                cx.run_until_parked();
            }
            if open.is_none() {
                use crate::settings::Section;
                let section = match *name {
                    "settings-general" => Section::General,
                    "settings-keybindings" => Section::Keybindings,
                    "settings-providers" => Section::Providers,
                    "settings-chat" => Section::Chat,
                    "settings-connections" => Section::Connections,
                    "settings-mcp" => Section::Mcp,
                    "settings-skills" => Section::Skills,
                    "settings-bots" => Section::Bots,
                    "settings-vox" => Section::Vox,
                    "settings-plugins" => Section::Connectors,
                    "settings-archive" => Section::Archive,
                    "settings-worktrees" => Section::Worktrees,
                    "settings-notes" => Section::Notes,
                    "settings-todos" => Section::Todos,
                    "settings-github" => Section::Github,
                    _ => Section::Appearance,
                };
                window
                    .update(&mut cx, |shell, window, cx| {
                        shell.open_settings(section, window, cx)
                    })
                    .unwrap();
                cx.run_until_parked();
            }
            window
                .update(&mut cx, |shell, window, cx| match *name {
                    "explorer" => shell.select_classic_sidebar(
                        crate::workspace::SidebarTab::Files,
                        window,
                        cx,
                    ),
                    "changes" => shell.select_classic_sidebar(
                        crate::workspace::SidebarTab::Changes,
                        window,
                        cx,
                    ),
                    "notes" => shell.open_notes(window, cx),
                    "automations" => shell.open_automations(window, cx),
                    "todos" => shell.open_todos(window, cx),
                    "quick-open" => shell.open_quick_open(false, window, cx),
                    "palette" => shell.open_quick_open(true, window, cx),
                    "diff" => shell.open_review(
                        crate::git_tools::review::ReviewTab::Changes {
                            cwd: PROJECT.into(),
                            path: Some(format!("{PROJECT}/src/spring.rs")),
                            staged: Some(false),
                        },
                        window,
                        cx,
                    ),
                    "file" => shell.open_review(
                        crate::git_tools::review::ReviewTab::File {
                            cwd: PROJECT.into(),
                            path: format!("{PROJECT}/src/spring.rs"),
                        },
                        window,
                        cx,
                    ),
                    "github" => {
                        shell.open_connector(crate::workspace::Connector::Github, window, cx)
                    }
                    "search" => shell.open_app_search(window, cx),
                    "approval-inline" => {
                        let view = shell.sessions[&sessions[0].id].clone();
                        view.update(cx, |view, cx| {
                            view.request_approval_for_test(
                                "capture",
                                "Run cargo nextest run -p vorkspace-gpui",
                            );
                            cx.notify();
                        });
                    }
                    "automation-editor" => {
                        shell.open_automations(window, cx);
                        if let Some(view) = shell.automations.clone() {
                            view.update(cx, |view, cx| view.begin_first_template(cx));
                        }
                    }
                    "project-appearance"
                    | "project-background"
                    | "project-codebase"
                    | "project-notifications" => {
                        use super::project_settings::ProjectSection;
                        let section = match *name {
                            "project-background" => ProjectSection::Background,
                            "project-codebase" => ProjectSection::Codebase,
                            "project-notifications" => ProjectSection::Notifications,
                            _ => ProjectSection::Appearance,
                        };
                        shell.open_project_settings(PROJECT.into(), section, window, cx)
                    }
                    "typed" => {
                        let view = shell.sessions[&sessions[0].id].clone();
                        view.update(cx, |view, cx| {
                            view.input.update(cx, |input, cx| {
                                input.set_text("Spring the composer's submit too".into(), cx)
                            })
                        });
                    }
                    "project-menu" => shell.open_menu(
                        MenuTarget::Project {
                            path: PROJECT.into(),
                            rail: true,
                        },
                        crate::popover::Anchor::Point(gpui::point(px(150.), px(600.))),
                        window,
                        cx,
                    ),
                    _ => {}
                })
                .unwrap();
            cx.run_until_parked();
            if name.starts_with("bot") {
                let new = *name == "bot-new";
                window
                    .update(&mut cx, |shell, w, cx| {
                        shell.workspace.right_panel_open = true;
                        let manager = shell.bot_manager();
                        manager.update(cx, |manager, cx| {
                            if new {
                                manager.create(vec![PROJECT.into()], w, cx)
                            } else {
                                manager.create_named(vec![PROJECT.into()], w, cx);
                                match name.strip_prefix("bot-") {
                                    Some(section) => manager.show_settings_section(section, cx),
                                    // The bot's own pane, not its settings.
                                    None => {
                                        manager.dismiss_settings(w, cx);
                                    }
                                }
                            }
                        });
                        cx.notify();
                    })
                    .unwrap();
                cx.run_until_parked();
            }
            // Diagrams rasterize off the main thread.
            std::thread::sleep(std::time::Duration::from_millis(300));
            cx.run_until_parked();
            // Through the window, not the Shell, so dispatch can update the Shell.
            cx.update_window(window.into(), |_, window, cx| {
                window.simulate_mouse_move(gpui::point(px(at.0), px(at.1)), cx)
            })
            .unwrap();
            cx.run_until_parked();
            vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
            if !matches!(*name, "live" | "glide") {
                // The first second may finish a last transition; after that
                // nothing should keep asking.
                let mut frames = 0;
                for step in 0..8 {
                    cx.advance_clock(std::time::Duration::from_millis(500));
                    let ran = cx
                        .update_window(window.into(), |_, window, cx| {
                            let ran = window.simulate_next_frame(cx);
                            window.draw(cx).clear(cx);
                            ran
                        })
                        .unwrap();
                    if step >= 2 {
                        frames += ran;
                    }
                    cx.run_until_parked();
                }
                if frames > 0 {
                    busy.push(format!(
                        "{theme}-{file}: {frames} frame callbacks in 3 s, idle"
                    ));
                }
            }
            if *name == "glide" {
                // Select the other tab and capture two frames into the glide.
                let id = sessions[0].id;
                window
                    .update(&mut cx, |shell, w, cx| shell.open(id, w, cx))
                    .unwrap();
                cx.run_until_parked();
                for _ in 0..3 {
                    cx.advance_clock(std::time::Duration::from_millis(16));
                    cx.update_window(window.into(), |_, window, cx| {
                        window.simulate_next_frame(cx);
                        window.refresh();
                        window.draw(cx).clear(cx);
                    })
                    .unwrap();
                }
            }
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(output.join(format!("{theme}-{file}.png")))
                .unwrap();
            if *name == "typed" {
                // Drafts persist per session; later shots start clean.
                window
                    .update(&mut cx, |shell, _, cx| {
                        let view = shell.sessions[&sessions[0].id].clone();
                        view.update(cx, |view, cx| {
                            view.input
                                .update(cx, |input, cx| input.set_text(String::new(), cx))
                        });
                    })
                    .unwrap();
                cx.run_until_parked();
            }
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
            if *name == "notice" {
                // The shell syncs approvals from live sessions, so the toast
                // is drawn on its own, in the theme just painted.
                use crate::notifications::approvals::{
                    ApprovalNotices, InputKind, PendingInput, SessionInputs,
                };
                let toast = cx
                    .open_window(gpui::size(px(400.), px(220.)), |_, cx| {
                        let notices = cx.new(|_| ApprovalNotices::default());
                        let sessions = [SessionInputs {
                            session_id: "capture".into(),
                            title: "Fix scroll anchor on resize".into(),
                            provider: "codex".into(),
                            harness: "Codex".into(),
                            pending: vec![PendingInput {
                                request_id: "capture".into(),
                                kind: InputKind::Approval,
                                label: "Run cargo nextest run -p vorkspace-gpui".into(),
                            }],
                            inbox_ask: false,
                            orchestration_worker: false,
                            conversation_focused: false,
                        }];
                        notices.update(cx, |notices, cx| notices.sync(&sessions, |_, _| true, cx));
                        notices
                    })
                    .unwrap();
                cx.run_until_parked();
                vorkspace_ui::clock::settle_headless_animations(toast, &mut cx);
                cx.capture_screenshot(toast.into())
                    .unwrap()
                    .save(output.join(format!("{theme}-approval.png")))
                    .unwrap();
                cx.update_window(toast.into(), |_, window, _| window.remove_window())
                    .unwrap();
            }
        }
    }
    std::fs::write(output.join("idle-frames.txt"), busy.join("\n")).unwrap();
    eprintln!("IDLE-FRAMES {}", busy.len());
    for line in &busy {
        eprintln!("IDLE-FRAMES {line}");
    }
}
