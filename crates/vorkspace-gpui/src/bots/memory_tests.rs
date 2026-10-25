use super::super::memory_ui::MemoryTarget;
use super::*;

const SHARED: &str = "- 2026-10-20 · The same saved fact";

fn save(s: &Setup, topic: Option<&str>, text: &str) {
    let path = topic
        .map(|name| format!("memory/{name}.md"))
        .unwrap_or_else(|| "MEMORY.md".into());
    bot::files::save(&s.root, &s.id, None, &path, text, None).unwrap();
}
fn read(s: &Setup, topic: Option<&str>) -> String {
    bot::memory::read(&s.root, &s.id, topic)
        .unwrap()
        .text
        .unwrap_or_default()
}
fn target(s: &Setup, topic: Option<&str>, line: &str) -> MemoryTarget {
    MemoryTarget {
        bot: s.id.clone(),
        topic: topic.map(str::to_owned),
        original: Some(line.into()),
    }
}
fn selector(prefix: &str, target: &MemoryTarget, index: usize) -> &'static str {
    Box::leak(format!("{prefix}-{}:{index}", target.key()).into_boxed_str())
}
fn show_memory(s: &Setup, cx: &mut TestAppContext) {
    cx.update(input::bind);
    s.window
        .update(cx, |_, window, _| window.activate_window())
        .unwrap();
    s.manager.update(cx, |m, cx| {
        m.settings = Some(SettingsSection::General);
        cx.notify();
    });
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    let memory = visual.debug_bounds("bot-settings-label-Memory").unwrap();
    visual.simulate_click(memory.center(), Default::default());
    visual.run_until_parked();
}
fn edit(visual: &mut gpui::VisualTestContext, target: &MemoryTarget, index: usize) {
    let row = visual
        .debug_bounds(selector("memory-fact-body", target, index))
        .unwrap();
    visual.simulate_click(row.center(), Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("bot-memory-editor").is_some());
}
fn draft(s: &Setup, visual: &mut gpui::VisualTestContext, text: &str) -> String {
    s.manager.update(visual, |m, cx| {
        let key = m.memory_editor.as_ref().unwrap().key.clone();
        m.fields[&key].update(cx, |f, cx| f.set_text(text.into(), cx));
        key
    })
}

#[gpui::test]
fn empty_root_shows_saved_topics_without_changing_prompt_memory(cx: &mut TestAppContext) {
    let s = setup(cx);
    save(
        &s,
        Some("architecture"),
        "- 2026-10-20 · Keep ownership explicit\n- 2026-10-21 · Publish state atomically\n",
    );
    save(
        &s,
        Some("development"),
        "- 2026-10-22 · Run focused checks\n",
    );
    save(&s, Some("ui"), "- 2026-10-23 · Preserve editor drafts\n");
    save(&s, Some("archive"), "- 2026-10-19 · Archived fact\n");
    show_memory(&s, cx);
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    for (width, height) in [(1400., 900.), (900., 650.)] {
        visual.simulate_resize(gpui::size(px(width), px(height)));
        visual.run_until_parked();
        assert!(visual.debug_bounds("bot-memory-empty").is_none());
        for (topic, line) in [
            ("architecture", "- 2026-10-20 · Keep ownership explicit"),
            ("development", "- 2026-10-22 · Run focused checks"),
            ("ui", "- 2026-10-23 · Preserve editor drafts"),
        ] {
            assert!(
                visual
                    .debug_bounds(selector(
                        "bot-memory-fact",
                        &target(&s, Some(topic), line),
                        0
                    ))
                    .is_some()
            );
        }
        assert!(
            visual
                .debug_bounds(selector(
                    "bot-memory-fact",
                    &target(&s, Some("archive"), "- 2026-10-19 · Archived fact"),
                    0
                ))
                .is_none()
        );
        let budget = visual.debug_bounds("Root memory · 0 / 200 lines").unwrap();
        let page = visual.debug_bounds("bot-memory-page").unwrap();
        assert!(budget.right() <= page.right() && budget.bottom() <= page.bottom());
    }
    s.manager.read_with(visual, |m, _| {
        let files = m.files.as_ref().unwrap();
        assert!(files.memory.is_empty());
        assert_eq!(m.memory_topics.len(), 3);
        let context = bot::context(
            m.roster.get(&s.id).unwrap(),
            files,
            std::path::Path::new("vorkspace"),
        );
        assert!(!context.contains("Keep ownership explicit"));
    });
    assert!(read(&s, None).is_empty());
}

#[gpui::test]
fn duplicate_facts_edit_and_delete_only_the_selected_source(cx: &mut TestAppContext) {
    let s = setup(cx);
    for topic in [None, Some("architecture"), Some("ui"), Some("archive")] {
        save(&s, topic, &format!("{SHARED}\n"));
    }
    show_memory(&s, cx);
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    edit(visual, &target(&s, Some("architecture"), SHARED), 0);
    draft(&s, visual, "Only architecture changes");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert!(visual.debug_bounds("bot-memory-editor").is_none());
    assert!(read(&s, Some("architecture")).contains("Only architecture changes"));
    for topic in [None, Some("ui"), Some("archive")] {
        assert_eq!(read(&s, topic), format!("{SHARED}\n"));
    }
    let forget = visual
        .debug_bounds(selector(
            "memory-forget",
            &target(&s, Some("ui"), SHARED),
            0,
        ))
        .unwrap();
    visual.simulate_mouse_move(forget.center(), None, Default::default());
    visual.simulate_click(forget.center(), Default::default());
    visual.run_until_parked();
    assert!(read(&s, Some("ui")).is_empty());
    assert_eq!(read(&s, None), format!("{SHARED}\n"));
    assert_eq!(read(&s, Some("archive")), format!("{SHARED}\n"));
    edit(visual, &target(&s, None, SHARED), 0);
    draft(&s, visual, "Root edit saves on blur");
    s.window
        .update(visual, |_, window, cx| {
            let focus = s.manager.read(cx).settings_focus.clone();
            window.focus(&focus, cx);
        })
        .unwrap();
    visual.run_until_parked();
    assert!(read(&s, None).contains("Root edit saves on blur"));
    assert!(read(&s, Some("architecture")).contains("Only architecture changes"));
}

#[gpui::test]
fn memory_cli_refreshes_existing_and_new_topics_and_reopening_settings_reloads(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    show_memory(&s, cx);
    assert!(
        call(
            &s,
            "topic-create",
            "memory.add",
            json!({"topic":"architecture","fact":"Visible from CLI"}),
            cx
        )
        .recv()
        .unwrap()
        .is_ok()
    );
    assert!(
        call(
            &s,
            "topic-update",
            "memory.add",
            json!({"topic":"architecture","fact":"Second CLI note"}),
            cx
        )
        .recv()
        .unwrap()
        .is_ok()
    );
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    let line = read(&s, Some("architecture"))
        .lines()
        .next()
        .unwrap()
        .to_owned();
    assert!(
        visual
            .debug_bounds(selector(
                "bot-memory-fact",
                &target(&s, Some("architecture"), &line),
                0
            ))
            .is_some()
    );
    s.manager.read_with(visual, |m, _| {
        assert!(
            m.memory_topics[0]
                .text
                .as_ref()
                .unwrap()
                .contains("Second CLI note")
        )
    });
    s.window
        .update(visual, |_, window, cx| {
            s.manager.update(cx, |m, cx| {
                m.dismiss_settings(window, cx);
            });
        })
        .unwrap();
    visual.run_until_parked();
    save(&s, Some("ui"), &format!("{SHARED}\n"));
    let settings = visual.debug_bounds("bot-settings").unwrap();
    visual.simulate_click(settings.center(), Default::default());
    visual.run_until_parked();
    let memory = visual.debug_bounds("bot-settings-label-Memory").unwrap();
    visual.simulate_click(memory.center(), Default::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(selector(
                "bot-memory-fact",
                &target(&s, Some("ui"), SHARED),
                0
            ))
            .is_some()
    );
    assert!(read(&s, None).is_empty());
}

#[gpui::test]
fn memory_empty_loading_and_source_errors_are_distinct_and_retry_keeps_saved_notes(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    show_memory(&s, cx);
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    assert!(visual.debug_bounds("bot-memory-empty").is_some());
    s.manager.update(visual, |m, cx| {
        m.memory_loading = true;
        cx.notify();
    });
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("bot-memory-loading").is_some());
    assert!(visual.debug_bounds("bot-memory-empty").is_none());
    save(&s, Some("architecture"), &format!("{SHARED}\n"));
    s.manager.update(visual, |m, cx| m.reload_files(&s.id, cx));
    visual.run_until_parked();
    let path = bot::files::load(&s.root, &s.id, None).unwrap().dir;
    std::fs::write(
        std::path::Path::new(&path).join("memory/architecture.md"),
        [0xff],
    )
    .unwrap();
    s.manager.update(visual, |m, cx| m.reload_files(&s.id, cx));
    visual.run_until_parked();
    assert!(visual.debug_bounds("bot-memory-source-error").is_some());
    assert!(visual.debug_bounds("bot-memory-empty").is_none());
    assert!(
        visual
            .debug_bounds(selector(
                "bot-memory-fact",
                &target(&s, Some("architecture"), SHARED),
                0
            ))
            .is_some()
    );
    save(&s, Some("architecture"), "- 2026-10-25 · Recovered note\n");
    let retry = visual.debug_bounds("bot-memory-retry").unwrap();
    visual.simulate_click(retry.center(), Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("bot-memory-source-error").is_none());
    let recovered = target(&s, Some("architecture"), "- 2026-10-25 · Recovered note");
    assert!(
        visual
            .debug_bounds(selector("bot-memory-fact", &recovered, 0))
            .is_some()
    );
    std::fs::write(std::path::Path::new(&path).join("MEMORY.md"), [0xff]).unwrap();
    s.manager.update(visual, |m, cx| m.reload_files(&s.id, cx));
    visual.run_until_parked();
    assert!(visual.debug_bounds("bot-memory-load-error").is_some());
    assert!(visual.debug_bounds("bot-memory-empty").is_none());
    assert!(
        visual
            .debug_bounds(selector("bot-memory-fact", &recovered, 0))
            .is_some()
    );
}

#[gpui::test]
fn stale_memory_line_and_duplicate_lines_keep_conflict_protection_and_drafts(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    save(&s, Some("architecture"), &format!("{SHARED}\n"));
    show_memory(&s, cx);
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    edit(visual, &target(&s, Some("architecture"), SHARED), 0);
    let key = draft(&s, visual, "Unsaved intent");
    save(&s, Some("architecture"), "- 2026-10-25 · CLI replacement\n");
    s.manager.update(visual, |m, cx| {
        m.reload_files(&s.id, cx);
        m.reload_files(&s.id, cx);
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("bot-memory-editor").is_some());
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    s.manager.read_with(visual, |m, cx| {
        assert!(m.error.as_ref().unwrap().contains("Memory changed"));
        assert_eq!(m.fields[&key].read(cx).content, "Unsaved intent");
        assert_eq!(m.memory_editor.as_ref().unwrap().key, key);
    });
    assert_eq!(
        read(&s, Some("architecture")),
        "- 2026-10-25 · CLI replacement\n"
    );
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    save(&s, Some("architecture"), &format!("{SHARED}\n{SHARED}\n"));
    s.manager.update(visual, |m, cx| m.reload_files(&s.id, cx));
    visual.run_until_parked();
    edit(visual, &target(&s, Some("architecture"), SHARED), 0);
    draft(&s, visual, "Ambiguous edit");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert_eq!(
        read(&s, Some("architecture")),
        format!("{SHARED}\n{SHARED}\n")
    );
    s.manager.read_with(visual, |m, _| {
        assert!(m.error.as_ref().unwrap().contains("Memory changed"))
    });
}

#[gpui::test]
fn pending_memory_saves_preserve_newer_drafts_and_source_switches(cx: &mut TestAppContext) {
    let s = setup(cx);
    for topic in [Some("architecture"), Some("ui")] {
        save(&s, topic, &format!("{SHARED}\n"));
    }
    show_memory(&s, cx);
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    edit(visual, &target(&s, Some("architecture"), SHARED), 0);
    let key = draft(&s, visual, "First submitted edit");
    // No executor progress until both the save and the newer input are set.
    s.manager.update(visual, |m, cx| {
        m.commit_memory_input(&key, false, cx);
        m.fields[&key].update(cx, |f, cx| f.set_text("Newer draft".into(), cx));
    });
    visual.run_until_parked();
    s.manager.read_with(visual, |m, cx| {
        let editor = m.memory_editor.as_ref().unwrap();
        assert_eq!(editor.key, key);
        assert!(
            editor
                .target
                .original
                .as_ref()
                .unwrap()
                .contains("First submitted edit")
        );
        assert_eq!(m.fields[&key].read(cx).content, "Newer draft");
    });
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert!(read(&s, Some("architecture")).contains("Newer draft"));
    edit(visual, &target(&s, Some("ui"), SHARED), 0);
    let old_key = draft(&s, visual, "UI submission");
    s.window
        .update(visual, |_, window, cx| {
            s.manager.update(cx, |m, cx| {
                m.commit_memory_input(&old_key, false, cx);
                m.begin_memory_editor(
                    MemoryTarget {
                        bot: s.id.clone(),
                        topic: None,
                        original: None,
                    },
                    window,
                    cx,
                );
                let key = m.memory_editor.as_ref().unwrap().key.clone();
                m.fields[&key].update(cx, |f, cx| f.set_text("New root draft".into(), cx));
                // A late blur or Enter from the replaced editor must do nothing.
                m.commit_memory_input(&old_key, true, cx);
            });
        })
        .unwrap();
    visual.run_until_parked();
    s.manager.read_with(visual, |m, cx| {
        let editor = m.memory_editor.as_ref().unwrap();
        assert!(editor.target.original.is_none());
        assert_eq!(m.fields[&editor.key].read(cx).content, "New root draft");
    });
    assert!(read(&s, Some("ui")).contains("UI submission"));
    assert!(read(&s, None).is_empty());
    s.manager.update(visual, |m, cx| {
        let key = m.memory_editor.as_ref().unwrap().key.clone();
        m.commit_memory_input(&key, true, cx);
        m.fields[&key].update(cx, |f, cx| f.set_text("Newer root draft".into(), cx));
    });
    visual.run_until_parked();
    assert!(read(&s, None).contains("New root draft"));
    s.manager.read_with(visual, |m, cx| {
        let editor = m.memory_editor.as_ref().unwrap();
        assert_eq!(m.fields[&editor.key].read(cx).content, "Newer root draft");
    });
}

#[gpui::test]
fn stale_memory_reload_and_save_cannot_touch_another_bot_editor(cx: &mut TestAppContext) {
    let s = setup(cx);
    save(&s, Some("architecture"), &format!("{SHARED}\n"));
    show_memory(&s, cx);
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    edit(visual, &target(&s, Some("architecture"), SHARED), 0);
    let key = draft(&s, visual, "Save to original Bot");
    s.window
        .update(visual, |_, window, cx| {
            s.manager.update(cx, |m, cx| {
                m.commit_memory_input(&key, false, cx);
                m.reload_files(&s.id, cx);
                m.roster.bots.push(Bot {
                    id: "memory-other".into(),
                    name: "Other".into(),
                    ..Default::default()
                });
                m.open("memory-other", window, cx);
                m.settings = Some(SettingsSection::Memory);
                m.reload_files("memory-other", cx);
                m.begin_memory_editor(
                    MemoryTarget {
                        bot: "memory-other".into(),
                        topic: None,
                        original: None,
                    },
                    window,
                    cx,
                );
                let next = m.memory_editor.as_ref().unwrap().key.clone();
                m.fields[&next].update(cx, |f, cx| f.set_text("Other Bot draft".into(), cx));
                m.commit_memory_input(&key, true, cx);
            });
        })
        .unwrap();
    visual.run_until_parked();
    s.manager.read_with(visual, |m, cx| {
        assert_eq!(m.files.as_ref().unwrap().id, "memory-other");
        assert!(m.memory_topics.is_empty());
        let editor = m.memory_editor.as_ref().unwrap();
        assert_eq!(editor.target.bot, "memory-other");
        assert_eq!(m.fields[&editor.key].read(cx).content, "Other Bot draft");
    });
    assert!(read(&s, Some("architecture")).contains("Save to original Bot"));
    assert!(
        bot::memory::read(&s.root, "memory-other", None)
            .unwrap()
            .text
            .unwrap()
            .is_empty()
    );
}

#[gpui::test]
fn topic_notes_keep_their_own_size_and_add_blur_writes_root(cx: &mut TestAppContext) {
    let s = setup(cx);
    let notes = (0..220)
        .map(|i| format!("- 2026-10-20 · Topic fact {i}\n"))
        .collect::<String>();
    save(&s, Some("architecture"), &notes);
    show_memory(&s, cx);
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    edit(
        visual,
        &target(&s, Some("architecture"), "- 2026-10-20 · Topic fact 0"),
        0,
    );
    draft(&s, visual, "Edited without a root budget");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert_eq!(read(&s, Some("architecture")).lines().count(), 220);
    assert!(read(&s, Some("archive")).is_empty());
    let add = visual.debug_bounds("bot-add-memory").unwrap();
    visual.simulate_click(add.center(), Default::default());
    visual.run_until_parked();
    draft(&s, visual, "Root note on blur");
    s.window
        .update(visual, |_, window, cx| {
            let focus = s.manager.read(cx).settings_focus.clone();
            window.focus(&focus, cx);
        })
        .unwrap();
    visual.run_until_parked();
    assert!(read(&s, None).contains("Root note on blur"));
    s.manager
        .read_with(visual, |m, _| assert!(m.memory_editor.is_none()));
}

#[gpui::test]
fn memory_budget_counts_only_root_lines_and_bytes(cx: &mut TestAppContext) {
    let s = setup(cx);
    let root = (0..205)
        .map(|i| format!("- Root fact {i}\n"))
        .collect::<String>();
    save(&s, None, &root);
    save(&s, Some("architecture"), &format!("{SHARED}\n"));
    show_memory(&s, cx);
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    assert!(
        visual
            .debug_bounds("Root memory · 200 / 200 lines · 5 lines not loading")
            .is_some()
    );
    let byte_limited = format!(
        "- Short root fact\n{}\n",
        "x".repeat(bot::memory::MAX_BYTES)
    );
    save(&s, None, &byte_limited);
    s.manager.update(visual, |m, cx| m.reload_files(&s.id, cx));
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("Root memory · 1 / 200 lines · 1 lines not loading")
            .is_some()
    );
    assert_eq!(read(&s, None), byte_limited);
    assert_eq!(read(&s, Some("architecture")), format!("{SHARED}\n"));
}
