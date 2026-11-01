use super::*;

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn ignored_names(entries: &[DirEntry]) -> Vec<String> {
    entries
        .iter()
        .filter(|e| e.ignored)
        .map(|e| e.name.clone())
        .collect()
}

fn is_ignored(dir: &Path, name: &str) -> bool {
    ignored_names(&list_dir(dir).unwrap())
        .iter()
        .any(|n| n == name)
}

#[test]
fn split_stem_ext_keeps_dotfiles_whole() {
    assert_eq!(split_stem_ext("foo.ts"), ("foo", ".ts"));
    assert_eq!(split_stem_ext("foo.d.ts"), ("foo.d", ".ts"));
    assert_eq!(split_stem_ext(".gitignore"), (".gitignore", ""));
    assert_eq!(split_stem_ext("Makefile"), ("Makefile", ""));
}

#[test]
fn create_rename_delete_and_copy_round_trip() {
    let dir = tmp();
    let dir = dir.path();
    let created = create_path(dir, "docs/notes.md", false).unwrap();
    assert_eq!(PathBuf::from(&created), dir.join("docs/notes.md"));
    assert!(create_path(dir, "docs/notes.md", false)
        .unwrap_err()
        .contains("notes.md already exists"));
    assert!(create_path(dir, "/abs", false)
        .unwrap_err()
        .contains("cannot start with a slash"));
    assert!(create_path(dir, "  /", true).is_err());
    assert!(create_path(dir, "a/../b", true)
        .unwrap_err()
        .contains("is not valid"));
    assert!(Path::new(&create_path(dir, "nested/folder/", true).unwrap()).is_dir());

    let file = dir.join("notes.md");
    std::fs::write(&file, "hi\n").unwrap();
    let renamed = rename_path(&file, "readme.md").unwrap();
    assert!(Path::new(&renamed).ends_with("readme.md"));
    assert!(!file.exists());
    assert_eq!(std::fs::read_to_string(&renamed).unwrap(), "hi\n");

    let copied = copy_path(Path::new(&renamed), dir).unwrap();
    assert!(Path::new(&copied).ends_with("readme copy.md"));
    let again = copy_path(Path::new(&renamed), dir).unwrap();
    assert!(Path::new(&again).ends_with("readme copy 2.md"));
    assert_eq!(std::fs::read_to_string(&copied).unwrap(), "hi\n");

    let nested = dir.join("docs");
    let moved = move_path(Path::new(&copied), &nested).unwrap();
    assert!(Path::new(&moved).ends_with("docs/readme copy.md"));
    assert!(!Path::new(&copied).exists());
    assert_eq!(move_path(Path::new(&moved), &nested).unwrap(), moved);

    delete_path(Path::new(&renamed)).unwrap();
    assert!(!Path::new(&renamed).exists());
    delete_path(&nested).unwrap();
    assert!(!nested.exists());
    assert!(delete_path(&nested)
        .unwrap_err()
        .ends_with("No such file or directory"));
}

#[test]
fn rename_refuses_collisions_and_moving_into_itself() {
    let dir = tmp();
    let dir = dir.path();
    std::fs::write(dir.join("a.txt"), "").unwrap();
    std::fs::write(dir.join("b.txt"), "").unwrap();
    std::fs::create_dir(dir.join("src")).unwrap();
    assert!(rename_path(&dir.join("a.txt"), "b.txt")
        .unwrap_err()
        .contains("b.txt already exists"));
    assert_eq!(
        rename_path(&dir.join("src"), "src/inner").unwrap_err(),
        "Cannot move a folder into itself."
    );
    let nested = rename_path(&dir.join("a.txt"), "deep/er/a.txt").unwrap();
    assert!(Path::new(&nested).is_file());
}

#[test]
fn copy_folder_gets_a_unique_name_and_rejects_paste_into_self() {
    let dir = tmp();
    let src = dir.path().join("src");
    std::fs::create_dir(&src).unwrap();
    std::fs::write(src.join("a.rs"), "fn a() {}\n").unwrap();
    let copied = copy_path(&src, dir.path()).unwrap();
    assert!(Path::new(&copied).ends_with("src copy"));
    assert!(Path::new(&copied).join("a.rs").exists());
    assert!(copy_path(&src, &src).unwrap_err().contains("itself"));
    assert!(move_path(&src, &src).unwrap_err().contains("itself"));
    assert!(copy_path(&src, &src.join("a.rs"))
        .unwrap_err()
        .ends_with("is not a folder"));
}

#[test]
fn copy_rejects_paste_into_self_through_a_symlink_alias() {
    let dir = tmp();
    let src = dir.path().join("src");
    std::fs::create_dir(&src).unwrap();
    std::fs::write(src.join("a.rs"), "fn a() {}\n").unwrap();
    let alias = dir.path().join("alias");
    std::os::unix::fs::symlink(&src, &alias).unwrap();
    assert!(copy_path(&src, &alias).unwrap_err().contains("itself"));
    assert!(move_path(&src, &alias).unwrap_err().contains("itself"));
}

#[test]
fn list_dir_sorts_numbered_names_naturally_with_folders_first() {
    let dir = tmp();
    for name in [
        "chapter-100.md",
        "chapter-11.md",
        "chapter-09.md",
        "chapter-10.md",
        "Chapter-2.md",
        ".DS_Store",
    ] {
        std::fs::write(dir.path().join(name), "").unwrap();
    }
    for name in ["volume-10", "volume-2"] {
        std::fs::create_dir(dir.path().join(name)).unwrap();
    }
    let entries = list_dir(dir.path()).unwrap();
    let names: Vec<_> = entries.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "volume-2",
            "volume-10",
            "Chapter-2.md",
            "chapter-09.md",
            "chapter-10.md",
            "chapter-11.md",
            "chapter-100.md",
        ]
    );
    assert!(entries[0].is_dir && !entries[2].is_dir);
    assert_eq!(entries[0].path, path_to_js(&dir.path().join("volume-2")));
}

#[test]
fn natural_name_sort_handles_multiple_and_large_numbers() {
    let mut names = [
        "part-2-chapter-10",
        "part-10-chapter-1",
        "part-2-chapter-2",
        "part-2-chapter-999999999999999999999999999999",
    ];
    names.sort_by(|a, b| compare_natural_names(a, b));
    assert_eq!(
        names,
        [
            "part-2-chapter-2",
            "part-2-chapter-10",
            "part-2-chapter-999999999999999999999999999999",
            "part-10-chapter-1",
        ]
    );
}

#[test]
fn list_dir_marks_ignored_with_git_semantics() {
    let dir = tmp();
    let dir = dir.path();
    let init = Command::new("git").args(["init"]).current_dir(dir).output();
    let Ok(init) = init else { return };
    if !init.status.success() {
        return;
    }
    std::fs::write(
        dir.join(".gitignore"),
        "*.zzlog\n!keep.zzlog\n/zz-dist\nzz-build/\nnested/*.zztmp\n",
    )
    .unwrap();
    std::fs::write(dir.join("a.zzlog"), "x\n").unwrap();
    std::fs::write(dir.join("keep.zzlog"), "x\n").unwrap();
    std::fs::write(dir.join("zz-build"), "x\n").unwrap();
    std::fs::create_dir_all(dir.join("zz-dist")).unwrap();
    std::fs::create_dir_all(dir.join("src").join("zz-dist")).unwrap();
    std::fs::create_dir_all(dir.join("nested")).unwrap();
    std::fs::write(dir.join("nested").join("x.zztmp"), "x\n").unwrap();
    std::fs::write(dir.join("nested").join("x.txt"), "x\n").unwrap();

    assert!(is_ignored(dir, ".git"));
    assert!(is_ignored(dir, "a.zzlog"));
    assert!(!is_ignored(dir, "keep.zzlog"));
    assert!(is_ignored(dir, "zz-dist"));
    assert!(!is_ignored(&dir.join("src"), "zz-dist"));
    assert!(!is_ignored(dir, "zz-build"));
    assert!(is_ignored(&dir.join("nested"), "x.zztmp"));
    assert!(!is_ignored(&dir.join("nested"), "x.txt"));
}

#[test]
fn list_dir_falls_back_to_name_parser_outside_git() {
    let dir = tmp();
    let dir = dir.path();
    std::fs::write(dir.join(".gitignore"), "secret.txt\n*.log\n").unwrap();
    std::fs::write(dir.join("secret.txt"), "x\n").unwrap();
    std::fs::write(dir.join("a.log"), "x\n").unwrap();
    std::fs::write(dir.join("app.ts"), "x\n").unwrap();
    let mut ignored = ignored_names(&list_dir(dir).unwrap());
    ignored.sort_unstable();
    assert_eq!(ignored, vec!["a.log", "secret.txt"]);
}

#[test]
fn list_dir_reports_missing_folder() {
    let dir = tmp();
    let missing = dir.path().join("gone");
    assert!(list_dir(&missing)
        .unwrap_err()
        .starts_with(&missing.display().to_string()));
}
