use super::*;
use crate::{bot::Roster, follow_up, store::Store, transcript::Row};
use serde_json::json;

fn bot(name: &str) -> Bot {
    Bot {
        name: name.into(),
        ..Roster::default().draft(vec!["/project".into()])
    }
}

#[test]
fn named_home_shares_files_and_keeps_its_identity_after_rename() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("bots");
    let mut bot = bot("Sam");
    let owner = super::super::load_files(&root, &bot).unwrap();
    files::save(&root, &bot.id, None, "MEMORY.md", "Keep this memory.", None).unwrap();
    let home = ensure(&root, &bot, None).unwrap();
    assert_eq!(home, temp.path().join("bot-workspaces/Sam"));
    assert_eq!(
        home.canonicalize().unwrap(),
        Path::new(&owner.dir).canonicalize().unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(home.join("MEMORY.md")).unwrap(),
        "Keep this memory."
    );
    std::fs::write(home.join("MEMORY.md"), "Edited outside Vorkspace.").unwrap();
    assert_eq!(
        super::super::load_files(&root, &bot).unwrap().memory,
        "Edited outside Vorkspace."
    );
    std::fs::write(home.join("AGENTS.md"), "My custom instructions.").unwrap();
    bot.name = "New name".into();
    assert_eq!(ensure(&root, &bot, Some(&home)).unwrap(), home);
    assert_eq!(
        std::fs::read_to_string(home.join("AGENTS.md")).unwrap(),
        "My custom instructions."
    );
    assert!(!temp.path().join("bot-workspaces/New name").exists());
}

#[test]
fn collisions_and_path_characters_never_replace_other_folders() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("bots");
    let first = bot("../Sam / 🐙");
    let second = bot(&first.name);
    let home = ensure(&root, &first, None).unwrap();
    assert_eq!(
        home.parent(),
        Some(temp.path().join("bot-workspaces").as_path())
    );
    let other = ensure(&root, &second, None).unwrap();
    assert_ne!(home, other);
    assert!(other
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .ends_with(&second.id));
    let third = bot("Occupied");
    let occupied = temp.path().join("bot-workspaces/Occupied");
    std::fs::create_dir(&occupied).unwrap();
    std::fs::write(occupied.join("keep"), "user content").unwrap();
    let third_home = ensure(&root, &third, None).unwrap();
    assert_ne!(third_home, occupied);
    assert_eq!(
        std::fs::read_to_string(occupied.join("keep")).unwrap(),
        "user content"
    );
    assert!(ensure(
        &root,
        &Bot {
            id: "../escape".into(),
            ..third
        },
        None
    )
    .is_err());
}

#[test]
fn session_pointer_uses_the_saved_account_and_cannot_be_overwritten_by_a_retired_session() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("bots");
    let bot = bot("Sam's Bot");
    let home = ensure(&root, &bot, None).unwrap();
    let store = Store::open(&temp.path().join("native.sqlite")).unwrap();
    let mut first = store.create("codex", home.to_str().unwrap()).unwrap();
    let account = Profile {
        id: "work".into(),
        root: None,
    };
    write_session(&root, &bot, &first, &account).unwrap();
    let path = home.join("SESSION.md");
    assert!(std::fs::read_to_string(&path)
        .unwrap()
        .contains("Send the Bot a message"));
    first.provider_id = Some("saved-thread".into());
    refresh_session(&root, &bot, &first, &account).unwrap();
    let pointer = std::fs::read_to_string(&path).unwrap();
    assert!(pointer.contains("codex resume --cd"));
    assert!(pointer.contains("provider-accounts/codex/work"));
    assert!(pointer.contains("env -u OPENAI_API_KEY -u CODEX_API_KEY -u CODEX_ACCESS_TOKEN"));
    let command = pointer
        .lines()
        .find(|line| line.starts_with("env "))
        .unwrap();
    let args = shlex::split(command).unwrap();
    assert!(args
        .windows(2)
        .any(|pair| pair == ["--cd", home.to_str().unwrap()]));
    assert!(args
        .windows(2)
        .any(|pair| pair == ["--add-dir", "/project"]));
    assert_eq!(&args[args.len() - 2..], &["--", "saved-thread"]);
    assert!(pointer.contains("- /project"));

    let second = store.create("codex", home.to_str().unwrap()).unwrap();
    write_session(&root, &bot, &second, &account).unwrap();
    let new_pointer = std::fs::read_to_string(&path).unwrap();
    refresh_session(&root, &bot, &first, &account).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), new_pointer);
    std::fs::write(&path, "My own session notes.").unwrap();
    write_session(&root, &bot, &second, &account).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "My own session notes."
    );
}

#[test]
fn legacy_codex_home_and_sqlite_configuration_travel_together() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("bots");
    let bot = bot("Legacy");
    let home = ensure(&root, &bot, None).unwrap();
    let provider_home = temp.path().join("codex-bots/default");
    std::fs::create_dir_all(&provider_home).unwrap();
    let store = Store::open(&temp.path().join("native.sqlite")).unwrap();
    let mut session = store.create("codex", home.to_str().unwrap()).unwrap();
    session.provider_id = Some("native-thread".into());
    write_session(&root, &bot, &session, &Profile::default()).unwrap();
    let text = std::fs::read_to_string(home.join("SESSION.md")).unwrap();
    let args = shlex::split(
        text.lines()
            .find(|line| line.starts_with("CODEX_HOME="))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(args[0], format!("CODEX_HOME={}", provider_home.display()));
    assert_eq!(
        args[1],
        format!("CODEX_SQLITE_HOME={}", provider_home.display())
    );
    assert_eq!(
        args[4],
        format!(
            "sqlite_home={}",
            serde_json::to_string(&provider_home).unwrap()
        )
    );
    assert_eq!(&args[5..7], &["resume", "--cd"]);
    assert_eq!(&args[args.len() - 2..], &["--", "native-thread"]);
}

#[test]
fn moving_a_bot_home_preserves_history_and_pending_input_across_reopen() {
    for shared in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("sessions.db");
        let open = || {
            if shared {
                Store::open_vorkspace(&path)
            } else {
                Store::open(&path)
            }
        };
        let mut store = open().unwrap();
        let mut session = store.create("codex", "/old-home").unwrap();
        store.provider_id(session.id, "keep-thread").unwrap();
        session.provider_id = Some("keep-thread".into());
        let row = Row {
            role: "user".into(),
            text: "Keep this chat".into(),
            first: true,
            sent_at: None,
        };
        store.write_rows(session.id, 0, &[row]).unwrap();
        let pending = follow_up::State {
            revision: 1,
            messages: vec![follow_up::Message {
                id: "queued".into(),
                text: "Send later".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        store.accept_follow_ups(session.id, &pending).unwrap();
        let draft = json!({"cwd":"/old-home", "text":"Unfinished", "selectionStart":10,"selectionEnd":10,"attachments":[],"mcpTags":[]});
        store.save_composer_draft(session.id, 0, &draft).unwrap();
        store.move_bot_home(&session, "/named-home").unwrap();
        assert!(store.move_bot_home(&session, "/stale-home").is_err());
        drop(store);
        let store = open().unwrap();
        let moved = store.bot_session(session.id).unwrap().unwrap();
        assert_eq!(moved.cwd, "/named-home");
        assert_eq!(moved.provider_id, session.provider_id);
        assert_eq!(
            store.load(session.id).unwrap().rows[0].text,
            "Keep this chat"
        );
        assert_eq!(
            store.follow_ups(session.id).unwrap().messages,
            pending.messages
        );
        assert_eq!(
            store.composer_draft(session.id).unwrap().unwrap().payload,
            draft
        );
        store.provider_id(session.id, "successor-thread").unwrap();
        assert!(store.move_bot_home(&moved, "/stale-provider").is_err());
        assert_eq!(
            store.bot_session(session.id).unwrap().unwrap().cwd,
            "/named-home"
        );
    }
}
