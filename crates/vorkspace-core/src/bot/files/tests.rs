
use super::*;

fn temp_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("bot-{}", crate::bot::new_id()));
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn creates_one_folder_per_bot_with_empty_memory() {
    let root = temp_root();
    let first = load(&root, "bot-a", None).unwrap();
    let again = load(&root, "bot-a", None).unwrap();
    let other = load(&root, "bot-b", None).unwrap();
    assert_eq!(first.id, "bot-a");
    assert_eq!(first.dir, again.dir);
    assert_ne!(first.dir, other.dir);
    assert_eq!(first.soul, None);
    assert_eq!(first.memory, "");
    assert!(Path::new(&first.dir).join("memory").is_dir());
    assert!(load(&root, "../x", None).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn indexed_project_owner_survives_load_edit_save_and_reopen_without_moving() {
    let root = temp_root();
    let old = root.join("old-id");
    std::fs::create_dir_all(old.join("memory")).unwrap();
    std::fs::write(old.join(MEMORY_FILE), "- kept").unwrap();
    let index = r#"{"agents":[{"id":"old-id","projects":["/code/app"],"opaque":true}],"future":{"keep":true}}"#;
    std::fs::write(root.join(INDEX_FILE), index).unwrap();
    let before = load(&root, "bot-a", Some("/code/app")).unwrap();
    assert_eq!(before.memory, "- kept");
    save(
        &root,
        "bot-a",
        Some("/code/app"),
        MEMORY_FILE,
        "- edited",
        Some(&before.memory_hash),
    )
    .unwrap();
    assert_eq!(
        load(&root, "bot-a", Some("/code/app")).unwrap().memory,
        "- edited"
    );
    assert_eq!(
        std::fs::read_to_string(root.join(INDEX_FILE)).unwrap(),
        index
    );
    assert_eq!(
        std::fs::read_to_string(old.join(MEMORY_FILE)).unwrap(),
        "- edited"
    );
    assert!(!root.join("bot-a").exists());
    let other = load(&root, "bot-b", None).unwrap();
    assert_eq!(other.memory, "");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn refuses_to_overwrite_a_changed_file() {
    let root = temp_root();
    let files = load(&root, "m", None).unwrap();
    let hash = save(
        &root,
        "m",
        None,
        MEMORY_FILE,
        "- a",
        Some(&files.memory_hash),
    )
    .unwrap();
    assert_eq!(
        save(
            &root,
            "m",
            None,
            MEMORY_FILE,
            "- b",
            Some(&files.memory_hash)
        ),
        Err(CONFLICT.into())
    );
    save(&root, "m", None, MEMORY_FILE, "- c", Some(&hash)).unwrap();
    save(&root, "m", None, SOUL_FILE, "soul", None).unwrap();
    let loaded = load(&root, "m", None).unwrap();
    assert_eq!(loaded.memory, "- c");
    assert_eq!(loaded.soul.as_deref(), Some("soul"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn writes_private_files_without_reusing_a_leftover_public_temp_file() {
    use std::os::unix::fs::PermissionsExt;

    let root = temp_root();
    let path = root.join(MEMORY_FILE);
    write_atomic(&path, "private memory").unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let temp = path.with_extension("md.tmp");
    std::fs::write(&temp, "leftover").unwrap();
    std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o644)).unwrap();
    write_atomic(&path, "new private memory").unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "new private memory"
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(std::fs::read_to_string(&temp).unwrap(), "leftover");
    assert_eq!(
        std::fs::metadata(&temp).unwrap().permissions().mode() & 0o777,
        0o644
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn concurrent_habit_claims_only_save_over_a_version_once() {
    use std::sync::{Arc, Barrier};

    let root = temp_root();
    let hash = save(&root, "m", None, HABITS_FILE, "[]", None).unwrap();
    let barrier = Arc::new(Barrier::new(8));
    let claims: Vec<_> = (0..8)
        .map(|index| {
            let root = root.clone();
            let hash = hash.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let text = format!("[{{\"claimedBy\":{index}}}]");
                barrier.wait();
                let result = save(&root, "m", None, HABITS_FILE, &text, Some(&hash));
                (text, result)
            })
        })
        .collect();
    let results: Vec<_> = claims
        .into_iter()
        .map(|claim| claim.join().unwrap())
        .collect();
    let winners: Vec<_> = results
        .iter()
        .filter(|(_, result)| result.is_ok())
        .collect();
    assert_eq!(winners.len(), 1);
    for (_, result) in results.iter().filter(|(_, result)| result.is_err()) {
        assert_eq!(result.as_ref().unwrap_err(), CONFLICT);
    }
    let saved = read(&root, "m", None, HABITS_FILE).unwrap();
    assert_eq!(saved.text.as_ref(), Some(&winners[0].0));
    assert_eq!(saved.hash, content_hash(&winners[0].0));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reads_and_writes_topics_inside_the_folder_only() {
    let root = temp_root();
    save(&root, "m", None, "memory/releases.md", "# Releases", None).unwrap();
    save(&root, "m", None, "memory/archive.md", "- old", None).unwrap();
    let loaded = load(&root, "m", None).unwrap();
    assert_eq!(loaded.topics, vec!["releases".to_string()]);
    let topic = read(&root, "m", None, "memory/releases.md").unwrap();
    assert_eq!(topic.text.as_deref(), Some("# Releases"));
    assert_eq!(read(&root, "m", None, "memory/none.md").unwrap().text, None);
    save(&root, "m", None, "habits.json", "[]", None).unwrap();
    for path in [
        "../x.md",
        "memory/../SOUL.md",
        "memory/a/b.md",
        "memory/.hidden.md",
        "notes.md",
    ] {
        assert!(save(&root, "m", None, path, "x", None).is_err(), "{path}");
    }
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn atomic_memory_write_never_follows_an_old_temporary_symlink() {
    let root = temp_root();
    let path = root.join(MEMORY_FILE);
    let sentinel = root.join("unrelated-private-file");
    std::fs::write(&sentinel, "keep untouched").unwrap();
    let old_temp = path.with_extension("md.tmp");
    std::os::unix::fs::symlink(&sentinel, &old_temp).unwrap();
    write_atomic(&path, "new memory").unwrap();
    assert_eq!(
        std::fs::read_to_string(&sentinel).unwrap(),
        "keep untouched"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "new memory");
    assert_eq!(std::fs::read_link(&old_temp).unwrap(), sentinel);
    assert!(!std::fs::symlink_metadata(&path)
        .unwrap()
        .file_type()
        .is_symlink());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_atomic_memory_publication_cleans_only_its_private_temporary_file() {
    let root = temp_root();
    let path = root.join(MEMORY_FILE);
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("keep"), "saved data").unwrap();
    assert!(write_atomic(&path, "cannot replace a directory").is_err());
    assert_eq!(
        std::fs::read_to_string(path.join("keep")).unwrap(),
        "saved data"
    );
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    write_atomic(&root.join(SOUL_FILE), "a subsequent save works").unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
