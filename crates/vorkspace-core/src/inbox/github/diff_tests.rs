use super::*;
use serde_json::{json, Value};

const TOO_LARGE: &str = "could not find pull request diff: HTTP 406: Sorry, the diff exceeded the maximum number of lines (20000)\nPullRequest.diff too_large";

fn no_local() -> Result<GitHubPrDiff, String> {
    Err("No local objects".into())
}

fn metadata() -> GitHubPrDiff {
    parse_github_pr_diff_meta(
        r#"{"additions":101,"deletions":0,"files":[{"path":"incomplete-metadata"}]}"#,
    )
    .unwrap()
}

fn file(path: &str) -> Value {
    json!({"filename":path, "status":"added", "additions":1, "deletions":0,
            "patch":"@@ -0,0 +1 @@\n+hello"})
}

#[test]
fn local_stats_include_every_file_and_preserve_rename_and_binary_paths() {
    let files = parse_git_pr_files(concat!(
        "1\t2\tspace\tname.rs\0-\t-\tbinary.png\0",
        "0\t0\t\0old.rs\0renamed\nλ.rs\0"
    ))
    .unwrap();
    assert_eq!(files.len(), 3);
    assert_eq!(files[0].path, "space\tname.rs");
    assert_eq!(files[1].additions, 0);
    assert_eq!(files[2].path, "renamed\nλ.rs");
    for invalid in ["1\t0", "1\t0\t\0old\0", "bad\t0\tfile\0"] {
        assert!(parse_git_pr_files(invalid).is_err());
    }
    assert!(parse_github_pr_oids(r#"{"baseRefOid":"--bad","headRefOid":"--bad"}"#).is_err());
}

#[test]
fn local_reconstruction_uses_merge_base_without_touching_checkout_or_index() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    };
    git(&["init", "-q"]);
    std::fs::write(root.join("a.rs"), "before\n").unwrap();
    std::fs::write(root.join("deleted.rs"), "gone\n").unwrap();
    git(&["add", "."]);
    let commit = [
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.com",
        "commit",
        "-qm",
        "test",
    ];
    git(&commit);
    let base = git(&["rev-parse", "HEAD"]);
    git(&["checkout", "-qb", "review"]);
    std::fs::write(root.join("a.rs"), "after\n").unwrap();
    std::fs::remove_file(root.join("deleted.rs")).unwrap();
    std::fs::write(root.join("new.rs"), "new\n").unwrap();
    git(&["add", "-A"]);
    git(&commit);
    let head = git(&["rev-parse", "HEAD"]);
    git(&["checkout", "-qb", "advanced", &base]);
    std::fs::write(root.join("outside.rs"), "outside PR\n").unwrap();
    git(&["add", "."]);
    git(&commit);
    let advanced = git(&["rev-parse", "HEAD"]);
    std::fs::write(root.join("a.rs"), "staged\n").unwrap();
    git(&["add", "a.rs"]);
    std::fs::write(root.join("a.rs"), "unsaved workspace\n").unwrap();
    let index = std::fs::read(root.join(".git/index")).unwrap();
    let mut diff = metadata();
    github_pr_hunks(
        &mut diff,
        "acme/app",
        "49",
        || git_pr_diff(root, &advanced, &head),
        |args| {
            assert_eq!(
                args[0], "pr",
                "local reconstruction must not call the files API"
            );
            Err(TOO_LARGE.into())
        },
    )
    .unwrap();
    assert_eq!(
        diff.files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["a.rs", "deleted.rs", "new.rs"]
    );
    assert_eq!((diff.additions, diff.deletions), (2, 2));
    assert!(!diff.truncated);
    assert!(diff.patch.contains("+after"));
    assert!(!diff.patch.contains("outside PR"));
    assert_eq!(
        git_pr_diff(root, &advanced, &head).unwrap().files,
        diff.files
    );
    assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
    assert_eq!(git(&["rev-parse", "HEAD"]), advanced);
    assert_eq!(
        std::fs::read_to_string(root.join("a.rs")).unwrap(),
        "unsaved workspace\n"
    );
}

#[test]
fn accepted_patch_with_capped_metadata_still_loads_the_complete_file_list() {
    let mut diff = metadata();
    diff.files = vec![diff.files[0].clone(); 100];
    github_pr_hunks(&mut diff, "acme/app", "49", no_local, |args| {
        if args[0] == "pr" {
            Ok("accepted patch".into())
        } else {
            Ok(json!([file("complete.rs")]).to_string())
        }
    })
    .unwrap();
    assert_eq!(diff.files.len(), 1);
    assert_eq!(diff.files[0].path, "complete.rs");
}

#[test]
fn ordinary_hunks_keep_the_fast_path_and_unrelated_errors_propagate() {
    let mut diff = metadata();
    github_pr_hunks(&mut diff, "acme/app", "49", no_local, |args| {
        assert_eq!(args, ["pr", "diff", "49", "--repo", "acme/app"]);
        Ok("ordinary patch".into())
    })
    .unwrap();
    assert_eq!(diff.patch, "ordinary patch");
    assert_eq!(diff.files[0].path, "incomplete-metadata");
    for error in [
        "HTTP 401: Bad credentials",
        "HTTP 406: Unsupported media type",
        "lookup timed out",
    ] {
        assert_eq!(
            github_pr_hunks(&mut diff, "acme/app", "49", no_local, |_| Err(error.into()))
                .unwrap_err(),
            error
        );
    }
}

#[test]
fn oversized_combined_diff_uses_all_file_pages_and_preserves_totals() {
    let mut diff = metadata();
    let first = (0..100)
        .map(|i| file(&format!("file-{i}.rs")))
        .collect::<Vec<_>>();
    let responses = [
        Err(TOO_LARGE.into()),
        Ok(serde_json::to_string(&first).unwrap()),
        Ok(json!([file("last.rs")]).to_string()),
    ];
    let mut responses = responses.into_iter();
    let mut calls = Vec::new();
    github_pr_hunks(&mut diff, "acme/app", "49", no_local, |args| {
        calls.push(args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>());
        responses.next().expect("unexpected request")
    })
    .unwrap();
    assert!(responses.next().is_none());
    assert_eq!(
        calls[1],
        ["api", "repos/acme/app/pulls/49/files?per_page=100&page=1"]
    );
    assert_eq!(
        calls[2],
        ["api", "repos/acme/app/pulls/49/files?per_page=100&page=2"]
    );
    assert_eq!(diff.files.len(), 101);
    assert_eq!(diff.additions, 101);
    assert_eq!(diff.files.last().unwrap().path, "last.rs");
    assert!(diff.patch.contains("+++ \"b/last.rs\"\n"));
    assert!(!diff.truncated);
}

#[test]
fn missing_and_incomplete_patches_are_reported_without_losing_other_files() {
    let mut diff = metadata();
    let rows = json!([
        {"filename":"missing.rs","status":"modified","additions":1,"deletions":0},
        {"filename":"partial.rs","status":"modified","additions":2,"deletions":0,"patch":"@@ -0,0 +1,2 @@\n+one"},
        file("present.rs")
    ]);
    github_pr_hunks(&mut diff, "acme/app", "49", no_local, |args| {
        if args[0] == "pr" {
            Err(TOO_LARGE.into())
        } else {
            Ok(rows.to_string())
        }
    })
    .unwrap();
    assert!(diff.truncated);
    assert_eq!(diff.files.len(), 3);
    assert!(!diff.patch.contains("missing.rs"));
    assert!(diff.patch.contains("+++ \"b/present.rs\""));
}

#[test]
fn byte_limit_skips_a_large_file_and_retains_later_small_patches() {
    let mut diff = metadata();
    let mut big = file("big.rs");
    big["patch"] = json!(format!("@@ -0,0 +1 @@\n+{}", "x".repeat(MAX_PR_DIFF_BYTES)));
    let rows = json!([big, file("small.rs")]).to_string();
    for oversized_output in [false, true] {
        github_pr_hunks(&mut diff, "acme/app", "49", no_local, |args| {
            if args[0] == "api" {
                Ok(rows.clone())
            } else if oversized_output {
                Err("GitHub CLI output exceeds the 4 MiB limit".into())
            } else {
                Ok("x".repeat(MAX_PR_DIFF_BYTES + 1))
            }
        })
        .unwrap();
        assert!(diff.truncated);
        assert_eq!(diff.files.len(), 2);
        assert!(diff.patch.len() <= MAX_PR_DIFF_BYTES);
        assert!(!diff.patch.contains("big.rs"));
        assert!(diff.patch.contains("small.rs"));
    }
}

#[test]
fn renamed_and_deleted_headers_preserve_paths_and_hunk_content() {
    let mut diff = metadata();
    let name = "space \"quoted\"/λ\nfile.rs";
    let rows = json!([
            {"filename":name,"previous_filename":"old.rs","status":"renamed","additions":1,"deletions":1,"patch":"@@ -1 +1 @@\n-old\n+new"},
            {"filename":"deleted.rs","status":"removed","additions":0,"deletions":1,"patch":"@@ -1 +0,0 @@\n-gone"}
        ]).to_string();
    github_pr_hunks(&mut diff, "acme/app", "49", no_local, |args| {
        if args[0] == "pr" {
            Err(TOO_LARGE.into())
        } else {
            Ok(rows.clone())
        }
    })
    .unwrap();
    assert_eq!(diff.files[0].path, name);
    assert!(diff.patch.contains(&format!(
        "--- \"a/old.rs\"\n+++ {}\n",
        serde_json::to_string(&format!("b/{name}")).unwrap()
    )));
    assert!(diff
        .patch
        .contains("--- \"a/deleted.rs\"\n+++ /dev/null\n@@ -1 +0,0 @@\n-gone"));
    assert!(!diff.truncated);
}

#[test]
fn file_api_cap_is_explicit_and_page_errors_do_not_become_success() {
    let mut diff = metadata();
    let page = serde_json::to_string(
        &(0..100)
            .map(|i| file(&format!("{i}.rs")))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let mut calls = 0;
    github_pr_hunks(&mut diff, "acme/app", "49", no_local, |args| {
        calls += 1;
        if args[0] == "pr" {
            Err(TOO_LARGE.into())
        } else {
            Ok(page.clone())
        }
    })
    .unwrap();
    assert_eq!(calls, 31);
    assert_eq!(diff.files.len(), 3000);
    assert!(diff.truncated);
    assert_eq!(
        github_pr_hunks(&mut diff, "acme/app", "49", no_local, |args| {
            Err(if args[0] == "pr" {
                TOO_LARGE
            } else {
                "HTTP 403: rate limited"
            }
            .into())
        })
        .unwrap_err(),
        "HTTP 403: rate limited"
    );
}
