use super::summary::{SUMMARY_OUTPUT_INSTRUCTIONS, chunks};
use super::*;
use recording::Owner;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
fn saved(directory: &Path) -> (Meetings, String) {
    let service = Meetings {
        directory: directory.into(),
    };
    let id = "sample".to_owned();
    let mut owner = Owner::create(directory, &id).unwrap();
    owner
        .append("microphone", 100, 1000, "We will ship Friday. 🦊".into())
        .unwrap();
    owner
        .append("system", 1100, 2200, "Sam owns the release.".into())
        .unwrap();
    owner.phase(recording::Phase::Stopped, None).unwrap();
    drop(owner);
    service
        .save(&Meetings::new_meeting(&id, "Planning".into()))
        .unwrap();
    atomic_json(
        &directory.join("settings.json"),
        &Settings {
            provider: "codex".into(),
            model: "fixture".into(),
            effort: None,
            prompt: DEFAULT_PROMPT.into(),
        },
    )
    .unwrap();
    (service, id)
}

fn draft_write(
    id: &str,
    expected: Option<&str>,
    version: &str,
    revision: u64,
    title: &str,
) -> DraftWrite {
    DraftWrite::new(
        id.into(),
        expected.map(str::to_owned),
        version.into(),
        Draft {
            revision,
            title: Some(title.into()),
            title_focused: true,
            ..Draft::default()
        },
    )
    .unwrap()
}

fn update(
    meetings: &Meetings,
    id: &str,
    revision: u64,
    edit: impl FnOnce(&mut Draft),
) -> Result<Meeting, String> {
    let mut draft = Draft {
        revision,
        ..Draft::default()
    };
    edit(&mut draft);
    let _lock = meetings.lock()?;
    meetings.update_locked(meetings.metadata(id)?, draft, None)
}

#[test]
fn resumed_takes_keep_corrected_text_and_invalidate_a_prior_summary() {
    use recording::{CaptureSource, Phase};

    let root = tempfile::tempdir().unwrap();
    let (meetings, id) = saved(root.path());
    let correction = "Corrected: we agreed to ship on Thursday. 🦊";
    update(&meetings, &id, 0, |draft| {
        draft.transcript = Some(correction.into())
    })
    .unwrap();
    let summary = meetings
        .summarize(&id, &AtomicBool::new(false), |_, _| {
            Ok("Prior summary".into())
        })
        .unwrap();
    assert_eq!(summary.meeting.summary.as_ref().unwrap().source_revision, 1);

    let mut first = meetings
        .start_recording(&id, "take-one", None, CaptureSource::Microphone)
        .unwrap();
    assert!(
        meetings
            .start_recording(&id, "take-two", Some("take-one"), CaptureSource::Microphone)
            .is_err()
    );
    assert!(first.append("system", 0, 400, "Forbidden".into()).is_err());
    let appended = first
        .append("microphone", 0, 300, "New detail.".into())
        .unwrap();
    assert_eq!(appended.sequence, 3);
    assert!(appended.start_ms >= 2200);
    first.phase(Phase::Stopped, None).unwrap();
    drop(first);

    let document = meetings.read(&id).unwrap();
    let text = document.transcript.as_str();
    assert!(text.starts_with(&format!("{correction}\n\n")));
    assert!(text.ends_with("New detail."));
    assert!(!text.contains("Sam owns the release"));
    assert_eq!(
        document.meeting.summary.as_ref().unwrap().text,
        "Prior summary"
    );
    assert_eq!(document.meeting.transcript_revision, 2);
    assert_eq!(
        document.meeting.summary.as_ref().unwrap().source_revision,
        1
    );
    assert!(
        meetings
            .start_recording(&id, "take-one", None, CaptureSource::Microphone)
            .is_err()
    );
    assert!(
        update(&meetings, &id, 2, |draft| draft.transcript =
            Some("Stale edit".into()))
        .is_err()
    );

    // A take admitted while a model is running invalidates its publication,
    // including when the new take has no final speech.
    let result = meetings.summarize(&id, &AtomicBool::new(false), |_, _| {
        let mut second = meetings
            .start_recording(
                &id,
                "take-two",
                Some("take-one"),
                CaptureSource::MicrophoneAndSystem,
            )
            .unwrap();
        assert_eq!(second.state.started_at, document.state.started_at);
        second.phase(Phase::Stopped, None).unwrap();
        Ok("Superseded output".into())
    });
    assert!(result.is_err());
    let recovered = meetings.read(&id).unwrap();
    assert_eq!(recovered.transcript, document.transcript);
    assert_eq!(recovered.meeting.transcript_revision, 2);
    assert_eq!(
        recovered.meeting.summary.as_ref().unwrap().text,
        "Prior summary"
    );
    assert_eq!(recovered.state.run.as_deref(), Some("take-two"));
}

#[test]
fn unrecorded_blank_checkpoints_and_empty_takes_stay_out_of_history() {
    use recording::{CaptureSource, Phase};

    let root = tempfile::tempdir().unwrap();
    let meetings = Meetings::open(root.path()).unwrap();
    let id = "new-meeting";
    assert!(meetings.read(id).is_err());
    meetings.prepare_document(id).unwrap();
    let dormant = meetings.open_document(id).unwrap();
    assert_eq!(dormant.meeting.title, "");
    assert_eq!(dormant.state.started_at, 0);
    assert!(
        meetings
            .note_candidates(&Default::default())
            .unwrap()
            .is_empty()
    );
    assert!(update(&meetings, id, 0, |draft| draft.title = Some("".into())).is_err());

    let first = "a".repeat(32);
    meetings
        .checkpoint_draft(&draft_write(id, None, &first, 0, "A draft"))
        .unwrap();
    assert_eq!(
        meetings.note_candidates(&Default::default()).unwrap()[0].title,
        "A draft"
    );
    assert!(
        meetings
            .start_recording(id, "blocked", None, CaptureSource::Microphone)
            .is_err()
    );
    let next = "b".repeat(32);
    meetings
        .checkpoint_draft(&draft_write(id, Some(&first), &next, 0, ""))
        .unwrap();
    meetings.save_draft(id, &next).unwrap();
    assert!(
        meetings
            .note_candidates(&Default::default())
            .unwrap()
            .is_empty()
    );

    let mut empty = meetings
        .start_recording(id, "empty", None, CaptureSource::Microphone)
        .unwrap();
    assert!(
        meetings
            .note_candidates(&Default::default())
            .unwrap()
            .is_empty()
    );
    empty.phase(Phase::Stopped, None).unwrap();
    drop(empty);
    assert!(
        meetings
            .note_candidates(&Default::default())
            .unwrap()
            .is_empty()
    );
    update(&meetings, id, 1, |draft| {
        draft.transcript = Some("Typed notes".into())
    })
    .unwrap();
    assert_eq!(
        meetings.note_candidates(&Default::default()).unwrap()[0].title,
        "Untitled meeting"
    );
    assert_eq!(meetings.read(id).unwrap().transcript, "Typed notes");
}

#[test]
fn a_resumed_take_cannot_overrun_the_corrected_document_limit() {
    use recording::CaptureSource;

    let root = tempfile::tempdir().unwrap();
    let (meetings, id) = saved(root.path());
    let text = "x".repeat(recording::MAX_TRANSCRIPT);
    update(&meetings, &id, 0, |draft| {
        draft.transcript = Some(text.as_str().into())
    })
    .unwrap();
    let mut owner = meetings
        .start_recording(&id, "too-long", None, CaptureSource::Microphone)
        .unwrap();
    assert!(
        owner
            .append("microphone", 0, 20, "A new line".into())
            .is_err()
    );
    assert_eq!(meetings.read(&id).unwrap().transcript.as_str(), text);
    assert_eq!(meetings.read(&id).unwrap().sequence, 2);
}

#[test]
fn meeting_draft_reopens_without_changing_the_saved_revision() {
    let root = tempfile::tempdir().unwrap();
    let (meetings, id) = saved(&root.path().join("meetings"));
    let version = "a".repeat(32);
    meetings
        .checkpoint_draft(&draft_write(&id, None, &version, 0, "Draft"))
        .unwrap();
    drop(meetings);

    let reopened = Meetings::open(root.path()).unwrap();
    let document = reopened.open_document(&id).unwrap();
    assert_eq!(document.meeting.title, "Planning");
    assert_eq!(document.meeting.revision, 0);
    assert_eq!(
        document.draft.as_ref().unwrap().title.as_deref().unwrap(),
        "Draft"
    );
    assert_eq!(document.draft.as_ref().unwrap().revision, 0);
    update(&reopened, &id, 0, |draft| {
        draft.title = Some("Changed elsewhere".into())
    })
    .unwrap();
    assert!(
        reopened
            .save_draft(&id, &version)
            .unwrap_err()
            .contains("changed")
    );
    let conflicted = reopened.open_document(&id).unwrap();
    assert_eq!(conflicted.meeting.title, "Changed elsewhere");
    assert_eq!(
        conflicted.draft.as_ref().unwrap().title.as_deref().unwrap(),
        "Draft"
    );
    assert_eq!(conflicted.draft.as_ref().unwrap().revision, 0);
}

#[test]
fn older_draft_writes_and_clears_cannot_replace_newer_input() {
    let root = tempfile::tempdir().unwrap();
    let (meetings, id) = saved(root.path());
    let older = "0123456789abcdef0123456700000001";
    let newer = "0123456789abcdef0123456700000003";
    let other = "c".repeat(32);
    let old_write = draft_write(&id, None, older, 0, "Old draft");
    let new_write = draft_write(&id, None, newer, 0, "Newest draft");
    meetings.checkpoint_draft(&old_write).unwrap();
    meetings.checkpoint_draft(&new_write).unwrap();
    assert!(meetings.checkpoint_draft(&old_write).is_err());
    assert!(
        meetings
            .checkpoint_draft(&draft_write(&id, Some(older), &other, 0, "Other window"))
            .is_err()
    );
    assert!(meetings.save_draft(&id, older).is_err());
    meetings.discard_draft(&id, older).unwrap();
    assert_eq!(
        meetings
            .open_document(&id)
            .unwrap()
            .draft
            .as_ref()
            .unwrap()
            .title
            .as_deref()
            .unwrap(),
        "Newest draft"
    );

    let checkpoint_path = root.path().join("drafts").join(format!("{id}.json"));
    let checkpoint = std::fs::read(&checkpoint_path).unwrap();
    assert_eq!(
        meetings.save_draft(&id, newer).unwrap().title,
        "Newest draft"
    );
    // Model a crash after canonical publication and before draft cleanup.
    std::fs::write(checkpoint_path, checkpoint).unwrap();
    assert!(meetings.open_document(&id).unwrap().draft.is_none());
    assert!(meetings.checkpoint_draft(&new_write).is_err());

    let next = draft_write(&id, Some(newer), &other, 1, "Next draft");
    meetings.checkpoint_draft(&next).unwrap();
    meetings.delete(&id, 1).unwrap();
    assert!(meetings.checkpoint_draft(&next).is_err());
    assert!(
        !root
            .path()
            .join("drafts")
            .join(format!("{id}.json"))
            .exists()
    );
}

#[test]
fn edit_lease_blocks_a_second_editor_until_the_owner_releases_it() {
    let root = tempfile::tempdir().unwrap();
    let (meetings, id) = saved(&root.path().join("meetings"));
    let second = Meetings::open(root.path()).unwrap();
    let lease = meetings.try_edit(&id).unwrap().unwrap();
    assert!(second.try_edit(&id).unwrap().is_none());
    assert_eq!(second.read(&id).unwrap().meeting.title, "Planning");
    drop(lease);
    assert!(second.try_edit(&id).unwrap().is_some());
}

#[test]
fn input_arriving_after_commit_advances_only_to_its_own_save_receipt() {
    let root = tempfile::tempdir().unwrap();
    let (meetings, id) = saved(root.path());
    let saved_version = "0123456789abcdef0123456700000001";
    // Save can start from a restored draft written by a previous editor.
    let late_version = "789abcdef0123456789abcde00000002";
    let latest_version = "789abcdef0123456789abcde00000003";
    meetings
        .checkpoint_draft(&draft_write(&id, None, saved_version, 0, "First"))
        .unwrap();
    meetings.save_draft(&id, saved_version).unwrap();
    meetings
        .checkpoint_draft(&draft_write(
            &id,
            Some(saved_version),
            late_version,
            0,
            "During save",
        ))
        .unwrap();
    assert_eq!(
        meetings
            .open_document(&id)
            .unwrap()
            .draft
            .as_ref()
            .unwrap()
            .revision,
        1
    );

    update(&meetings, &id, 1, |draft| {
        draft.title = Some("External update".into())
    })
    .unwrap();
    meetings
        .checkpoint_draft(&draft_write(
            &id,
            Some(saved_version),
            latest_version,
            0,
            "Latest edit",
        ))
        .unwrap();
    let document = meetings.open_document(&id).unwrap();
    assert_eq!(document.meeting.title, "External update");
    assert_eq!(document.draft.as_ref().unwrap().revision, 1);
    assert_eq!(
        document.draft.as_ref().unwrap().title.as_deref().unwrap(),
        "Latest edit"
    );
    assert!(meetings.save_draft(&id, latest_version).is_err());
}

#[test]
fn escaped_edits_at_raw_limits_reopen_after_save() {
    let root = tempfile::tempdir().unwrap();
    let (meetings, id) = saved(root.path());
    meetings
        .summarize(&id, &AtomicBool::new(false), |_, _| Ok("Original".into()))
        .unwrap();
    let version = "b".repeat(32);
    let transcript = "\n".repeat(recording::MAX_TRANSCRIPT);
    let summary = "\u{1}".repeat(MAX_SUMMARY_BYTES);
    let write = DraftWrite::new(
        id.clone(),
        None,
        version.clone(),
        Draft {
            revision: 1,
            transcript: Some(transcript.as_str().into()),
            summary: Some(summary.as_str().into()),
            ..Draft::default()
        },
    )
    .unwrap();
    meetings.checkpoint_draft(&write).unwrap();
    assert_eq!(
        meetings
            .open_document(&id)
            .unwrap()
            .draft
            .as_ref()
            .unwrap()
            .transcript
            .as_deref(),
        Some(transcript.as_str())
    );
    meetings.save_draft(&id, &version).unwrap();
    let reopened = meetings.open_document(&id).unwrap();
    assert_eq!(reopened.transcript, transcript);
    assert_eq!(
        reopened
            .meeting
            .summary
            .as_ref()
            .map(|summary| summary.text.as_str()),
        Some(summary.as_str())
    );
}

#[test]
fn nested_symlinks_cannot_redirect_meeting_storage() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::set_permissions(outside.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let (meetings, id) = saved(root.path());
    for child in ["meetings", "recordings"] {
        let nested = root.path().join(child);
        let backup = root.path().join(format!("{child}-backup"));
        std::fs::rename(&nested, &backup).unwrap();
        symlink(outside.path(), &nested).unwrap();
        assert!(meetings.read(&id).is_err());
        if child == "meetings" {
            assert!(meetings.prepare("Redirected").is_err());
            assert!(
                update(&meetings, &id, 0, |draft| draft.title =
                    Some("Redirected".into()))
                .is_err()
            );
            assert_eq!(
                meetings.note_candidates(&Default::default()).unwrap()[0].title,
                "Unreadable meeting"
            );
        } else {
            assert!(meetings.note_candidates(&Default::default()).is_err());
            assert!(recording::state(root.path(), &id).is_err());
            assert!(Owner::create(root.path(), "new").is_err());
        }
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
        assert_eq!(
            std::fs::metadata(outside.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        std::fs::remove_file(&nested).unwrap();
        std::fs::rename(&backup, &nested).unwrap();
    }
}

#[test]
fn deleting_checks_the_current_revision_and_prevents_late_publication() {
    let root = tempfile::tempdir().unwrap();
    let (meetings, id) = saved(root.path());
    update(&meetings, &id, 0, |draft| {
        draft.title = Some("Latest".into())
    })
    .unwrap();
    assert!(meetings.delete(&id, 0).unwrap_err().contains("changed"));
    assert_eq!(meetings.read(&id).unwrap().meeting.title, "Latest");

    let late = meetings.summarize(&id, &AtomicBool::new(false), |_, _| {
        meetings.delete(&id, 1).unwrap();
        Ok("Do not recreate the meeting".into())
    });
    assert!(late.is_err());
    assert!(
        update(&meetings, &id, 1, |draft| draft.title =
            Some("Stale".into()))
        .is_err()
    );
    assert!(meetings.read(&id).is_err());
    assert!(
        meetings
            .note_candidates(&Default::default())
            .unwrap()
            .is_empty()
    );
    assert!(!meetings.metadata_path(&id).unwrap().exists());
    assert_eq!(
        std::fs::read_dir(root.path().join("recordings"))
            .unwrap()
            .count(),
        0
    );
    meetings.delete(&id, 1).unwrap();
}

#[test]
fn deleting_waits_for_the_recorder_to_release_the_transcript() {
    let root = tempfile::tempdir().unwrap();
    let meetings = Meetings {
        directory: root.path().into(),
    };
    let id = meetings.prepare("Capture").unwrap();
    let mut owner = Owner::create(root.path(), &id).unwrap();
    assert!(
        meetings
            .delete(&id, 0)
            .unwrap_err()
            .contains("Stop recording")
    );
    owner.phase(recording::Phase::Stopped, None).unwrap();
    assert!(meetings.delete(&id, 0).is_err());
    drop(owner);
    meetings.delete(&id, 0).unwrap();
    assert!(
        meetings
            .note_candidates(&Default::default())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn listing_completes_cleanup_after_a_recording_has_been_retired() {
    let root = tempfile::tempdir().unwrap();
    let (meetings, id) = saved(root.path());
    let live = recording::directory(root.path(), &id).unwrap();
    let retired = live.with_file_name(format!(".deleted-{id}"));
    std::fs::create_dir(&retired).unwrap();
    assert_eq!(
        meetings.note_candidates(&Default::default()).unwrap().len(),
        1
    );
    assert!(meetings.read(&id).is_ok());
    std::fs::remove_dir(&retired).unwrap();
    std::fs::rename(&live, &retired).unwrap();
    assert!(
        update(&meetings, &id, 0, |draft| draft.title =
            Some("Stale".into()))
        .is_err()
    );
    assert!(
        meetings
            .note_candidates(&Default::default())
            .unwrap()
            .is_empty()
    );
    assert!(!retired.exists());
    assert!(!meetings.metadata_path(&id).unwrap().exists());
}

#[test]
fn abandoned_recording_keeps_text_without_inventing_an_end_time() {
    let root = tempfile::tempdir().unwrap();
    let mut owner = Owner::create(root.path(), "sample").unwrap();
    owner.state.started_at = recording::now_ms() - 3_600_000;
    owner.phase(recording::Phase::Recording, None).unwrap();
    owner
        .append("system", 0, 2000, "Saved speech".into())
        .unwrap();
    drop(owner);

    let saved = recording::read(root.path(), "sample").unwrap();
    assert_eq!(saved.state.phase, recording::Phase::Interrupted);
    assert_eq!(saved.state.ended_at, None);
    assert_eq!(saved.segments[0].text, "Saved speech");
}

#[test]
fn canceled_summary_cannot_publish_generated_text() {
    let root = tempfile::tempdir().unwrap();
    let (meetings, id) = saved(root.path());
    let cancel = AtomicBool::new(false);
    let result = meetings.summarize(&id, &cancel, |_, _| {
        cancel.store(true, Ordering::Release);
        Ok("Do not publish".into())
    });
    assert!(result.unwrap_err().contains("canceled"));
    assert_eq!(meetings.read(&id).unwrap().meeting.revision, 0);
    assert!(meetings.read(&id).unwrap().meeting.summary.is_none());
}

#[test]
fn meeting_list_excludes_documents_but_reads_keep_saved_content() {
    let root = tempfile::tempdir().unwrap();
    let (meetings, id) = saved(root.path());
    update(&meetings, &id, 0, |draft| {
        draft.transcript = Some("Edited transcript".into())
    })
    .unwrap();
    meetings
        .summarize(&id, &AtomicBool::new(false), |_, _| {
            Ok("Summary text".into())
        })
        .unwrap();

    let list = meetings.note_candidates(&Default::default()).unwrap();
    assert_eq!(list[0].recording_id, id);
    assert_eq!(list[0].title, "Planning");
    assert!(
        meetings
            .note_candidates(&[id.clone()].into())
            .unwrap()
            .is_empty()
    );
    let doc = meetings.read(&id).unwrap();
    assert_eq!(doc.transcript, "Edited transcript");
    assert_eq!(doc.meeting.summary.as_ref().unwrap().text, "Summary text");
}

#[test]
fn old_summary_settings_load_without_an_effort_override() {
    let root = tempfile::tempdir().unwrap();
    let service = Meetings::open(root.path()).unwrap();
    atomic_json(
        &service.directory.join("settings.json"),
        &json!({
            "provider": "codex", "model": "existing-model", "prompt": "My summary format"
        }),
    )
    .unwrap();
    let settings = service.settings().unwrap();
    assert_eq!(settings.effort, None);
    assert_eq!(settings.model, "existing-model");
    assert_eq!(settings.prompt, "My summary format");
}

#[test]
fn summary_round_trip_preserves_original_and_uses_selected_model_and_effort() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("meetings");
    let (service, id) = saved(&data);
    let mut settings = service.settings().unwrap();
    settings.effort = Some("high".into());
    service.save_settings(&settings).unwrap();
    let service = Meetings::open(dir.path()).unwrap();
    let before = recording::read(&data, &id).unwrap().segments;
    let result = service
        .summarize(&id, &AtomicBool::new(false), |settings, prompt| {
            assert_eq!(settings.model, "fixture");
            assert_eq!(settings.effort.as_deref(), Some("high"));
            assert!(prompt.contains("Sam owns"));
            Ok("## Action items\n- Sam: ship Friday.".into())
        })
        .unwrap();
    assert_eq!(
        result.meeting.summary.as_ref().unwrap().text,
        "## Action items\n- Sam: ship Friday."
    );
    assert_eq!(recording::read(&data, &id).unwrap().segments, before);
    let changed = update(&service, &id, 1, |draft| {
        draft.title = Some("Renamed".into())
    })
    .unwrap();
    assert_eq!(
        changed.summary.as_ref().unwrap().text,
        "## Action items\n- Sam: ship Friday."
    );
    assert!(changed.transcript_override.is_none());
    let corrected = "## Corrected\n- Keep my wording.";
    let edited = update(&service, &id, 2, |draft| {
        draft.summary = Some(corrected.into())
    })
    .unwrap();
    assert_eq!(edited.summary.as_ref().unwrap().text, corrected);
    let reopened = service.read(&id).unwrap();
    assert_eq!(reopened.meeting.summary.as_ref().unwrap().text, corrected);
    assert_eq!(recording::read(&data, &id).unwrap().segments, before);
}
#[test]
fn custom_notes_prompt_keeps_mandatory_final_content_rules_without_rewriting_settings() {
    let dir = tempfile::tempdir().unwrap();
    let (service, id) = saved(dir.path());
    let mut settings = service.settings().unwrap();
    settings.prompt = "Write action items, and include your reasoning and work steps.".into();
    service.save_settings(&settings).unwrap();
    service
        .summarize(&id, &AtomicBool::new(false), |_, prompt| {
            assert!(prompt.starts_with(SUMMARY_OUTPUT_INSTRUCTIONS));
            assert!(prompt.contains(&settings.prompt));
            Ok("## Action items\n- Sam: ship Friday.".into())
        })
        .unwrap();
    assert_eq!(service.settings().unwrap().prompt, settings.prompt);
}
#[test]
fn failed_or_empty_regeneration_preserves_existing_user_edited_notes() {
    let dir = tempfile::tempdir().unwrap();
    let (service, id) = saved(dir.path());
    service
        .summarize(&id, &AtomicBool::new(false), |_, _| {
            Ok("Original notes".into())
        })
        .unwrap();
    update(&service, &id, 1, |draft| {
        draft.summary = Some("My wording: keep this exactly.".into())
    })
    .unwrap();
    let before = service.read(&id).unwrap();
    for generated in [Err("Turn failed after final".into()), Ok("  ".into())] {
        let mut generated = Some(generated);
        assert!(
            service
                .summarize(&id, &AtomicBool::new(false), |_, _| generated
                    .take()
                    .unwrap())
                .is_err()
        );
        assert_eq!(service.read(&id).unwrap(), before);
    }
}
#[test]
fn an_edit_during_ai_cannot_be_overwritten_and_failure_retains_the_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let (service, id) = saved(dir.path());
    let error = service
        .summarize(&id, &AtomicBool::new(false), |_, _| {
            update(&service, &id, 0, |draft| {
                draft.transcript = Some("My corrected transcript".into())
            })
            .unwrap();
            Ok("stale".into())
        })
        .unwrap_err();
    assert!(error.contains("changed while"));
    let saved = service.read(&id).unwrap();
    assert_eq!(saved.transcript, "My corrected transcript");
    assert!(saved.meeting.summary.is_none());
    assert!(
        service
            .summarize(&id, &AtomicBool::new(false), |_, _| Err(
                "provider unavailable".into()
            ))
            .is_err()
    );
    assert_eq!(
        service.read(&id).unwrap().transcript,
        "My corrected transcript"
    );
}
#[test]
fn long_inputs_shrink_in_bounded_passes_and_unicode_boundaries_survive() {
    let text = "🦊 action item\n".repeat(6000);
    let parts = chunks(&text, 32 * 1024);
    assert_eq!(parts.join(""), text);
    assert!(parts.iter().all(|p| p.len() <= 32 * 1024));
    let dir = tempfile::tempdir().unwrap();
    let (service, id) = saved(dir.path());
    update(&service, &id, 0, |draft| {
        draft.transcript = Some(text.into())
    })
    .unwrap();
    let mut settings = service.settings().unwrap();
    settings.prompt = "Preserve discussion about battery testing.".into();
    service.save_settings(&settings).unwrap();
    let mut calls = 0;
    let result = service
        .summarize(&id, &AtomicBool::new(false), |_, prompt| {
            calls += 1;
            assert!(prompt.len() < 40 * 1024);
            assert!(prompt.contains("Preserve discussion about battery testing."));
            assert!(prompt.starts_with(SUMMARY_OUTPUT_INSTRUCTIONS));
            Ok("- Sam owns release.".into())
        })
        .unwrap();
    assert!(calls > 1);
    assert!(result.meeting.summary.is_some());
}
