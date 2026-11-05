use super::*;
use gpui::{HeadlessAppContext, Render, size};
use std::sync::{Arc, Mutex};
use vorkspace_core::{notes::NoteUpsert, store::Store};

struct Surface(Entity<Notes>);

impl Render for Surface {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(pane_fill())
            .font_family(vorkspace_ui::fonts::UI)
            .child(self.0.clone())
    }
}

#[test]
#[ignore = "Requires Metal and VORKSPACE_PAGES_SCREEN_DIR"]
fn native_notes_capture() {
    let output = PathBuf::from(std::env::var_os("VORKSPACE_PAGES_SCREEN_DIR").unwrap());
    assert!(output.is_absolute() && output.is_dir());
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("notes.db")).unwrap();
    let meetings = vorkspace_meetings::Meetings::open(root.path()).unwrap();
    let recording_id = meetings.prepare("Release planning").unwrap();
    let mut owner =
        vorkspace_meetings::recording::Owner::create(&meetings.directory, &recording_id).unwrap();
    owner
        .append(
            "microphone",
            0,
            1800,
            "Taylor will run the smoke tests before Friday.".into(),
        )
        .unwrap();
    owner
        .append(
            "system",
            2200,
            3800,
            "Morgan will review the release notes.".into(),
        )
        .unwrap();
    owner
        .phase(vorkspace_meetings::recording::Phase::Stopped, None)
        .unwrap();
    drop(owner);
    meetings
        .save_settings(&vorkspace_meetings::Settings {
            provider: "codex".into(),
            model: "fixture".into(),
            ..Default::default()
        })
        .unwrap();
    meetings.summarize(&recording_id, &std::sync::atomic::AtomicBool::new(false), |_, _| {
        Ok("## Summary\n\nThe release is ready for final validation.\n\n## Decisions\n\n- Complete smoke tests before publishing.\n\n## Action items\n\n- **Taylor:** run smoke tests before Friday.\n- **Morgan:** review the release notes.".into())
    }).unwrap();
    let meeting_note = store
        .import_recording_note(&recording_id, "Release planning", 1793836800000)
        .unwrap()
        .unwrap();
    let project = "/work/fixtures/release-platform";
    let note = store
        .upsert_note(&NoteUpsert {
            id: "release-checklist".into(),
            title: "Release checklist".into(),
            body: "## Before publishing\n\nRun the **smoke tests** against the signed desktop build.\n\n- Verify the upgrade path.\n- Confirm the help links.\n\n| Check | Owner |\n| --- | --- |\n| macOS upgrade | Taylor |\n| Release notes | Morgan |\n\n```sh\ncargo test --workspace\n```\n".into(),
            tags: vec!["release".into(), "quality".into()],
            source_cwd: Some(project.into()),
            finalize_slug: true,
            ..Default::default()
        })
        .unwrap();
    let follow_up = store
        .upsert_note(&NoteUpsert {
            id: "platform-follow-up".into(),
            title: "Platform follow-up".into(),
            body: "Record remaining launch checks and assign owners before review.".into(),
            tags: vec!["platform".into()],
            source_cwd: Some(project.into()),
            finalize_slug: true,
            ..Default::default()
        })
        .unwrap();
    let store = Arc::new(Mutex::new(store));
    let mut cx = HeadlessAppContext::with_platform(
        Arc::new(gpui_apple::AppleTextSystem::new()),
        Arc::new(crate::assets::Assets),
        || {
            Ok(Some(Box::new(
                gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
            )))
        },
    );
    for theme in ["dark", "light"] {
        cx.update(|_| {
            configure(
                &vorkspace_ui::preferences::Preferences {
                    theme: theme.into(),
                    ..Default::default()
                },
                theme == "light",
                true,
            );
        });
        for (layout, width) in [("wide", 1200.), ("narrow", 540.)] {
            for section in ["preview", "source", "summary", "transcript"] {
                let window = cx
                    .open_window(size(px(width), px(780.)), |_, cx| {
                        cx.new(|cx| {
                            Surface(cx.new(|cx| {
                                let mut view = Notes::new(store.clone(), cx);
                                // Publish the selected fixture immediately. Any
                                // queued constructor load has the older revision.
                                view.revision += 1;
                                view.loading = false;
                                view.cwd = project.into();
                                view.projects = vec![project.into()];
                                view.notes =
                                    vec![meeting_note.clone(), note.clone(), follow_up.clone()];
                                view.recordings
                                    .insert(meeting_note.id.clone(), recording_id.clone());
                                let recording = matches!(section, "summary" | "transcript");
                                view.load_selection(
                                    Some(if recording {
                                        meeting_note.id.clone()
                                    } else {
                                        note.id.clone()
                                    }),
                                    cx,
                                );
                                view.source = section == "source";
                                view
                            }))
                        })
                    })
                    .unwrap();
                cx.run_until_parked();
                window
                    .update(&mut cx, |surface, _, cx| {
                        surface.0.update(cx, |view, cx| {
                            if matches!(section, "summary" | "transcript") {
                                view.show_recording = true;
                                view.recording.update(cx, |page, cx| {
                                    page.note_section(section == "transcript", cx)
                                });
                            }
                        });
                    })
                    .unwrap();
                vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(output.join(format!("notes-{theme}-{layout}-{section}.png")))
                    .unwrap();
                cx.update_window(window.into(), |_, window, _| window.remove_window())
                    .unwrap();
            }
        }
    }
}
