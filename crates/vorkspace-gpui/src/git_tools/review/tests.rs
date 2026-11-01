use super::*;
struct Repo(PathBuf);
impl Repo {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "vorkspace-review-cache-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let repo = Self(root);
        repo.git(&["init"]);
        for name in ["a.txt", "b.txt"] {
            std::fs::write(repo.0.join(name), "base\n").unwrap();
        }
        repo.git(&["add", "."]);
        repo.git(&["commit", "-m", "base"]);
        for name in ["a.txt", "b.txt"] {
            std::fs::write(repo.0.join(name), "edit1\n").unwrap();
        }
        repo
    }
    fn git(&self, args: &[&str]) {
        let result = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(&self.0)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    fn cwd(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}
impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[gpui::test]
fn review_focus_follows_the_visible_body_after_loading(cx: &mut gpui::TestAppContext) {
    let repo = Repo::new();
    let cwd = repo.cwd();
    let preview = repo.0.join("preview.md");
    std::fs::write(&preview, "# Preview\n").unwrap();
    let window = cx.update(|cx| {
        crate::input::bind(cx);
        cx.open_window(Default::default(), |_, cx| {
            let git = cx.new(|cx| ProjectGit::new(cwd.clone(), cx));
            cx.new(|cx| ReviewPane::new(git, Default::default(), cx))
        })
        .unwrap()
    });
    window
        .update(cx, |pane, w, cx| {
            pane.set_deferred(true, cx);
            pane.open(
                ReviewTab::Changes {
                    cwd: cwd.clone(),
                    path: None,
                    staged: None,
                },
                cx,
            );
            pane.set_visible(true, cx);
            pane.focus_active(w, cx);
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    window
        .update(visual, |pane, w, cx| {
            assert!(pane.focus.is_focused(w), "the deferred pane retains focus");
            pane.set_deferred(false, cx);
        })
        .unwrap();
    visual.run_until_parked();
    window
        .update(visual, |pane, w, cx| {
            assert!(pane.sources[0].view.focus_handle(cx).is_focused(w));
        })
        .unwrap();
    for path in [repo.0.join("a.txt"), preview] {
        window
            .update(visual, |pane, w, cx| {
                pane.open(
                    ReviewTab::File {
                        cwd: cwd.clone(),
                        path: path.to_string_lossy().into_owned(),
                    },
                    cx,
                );
                pane.focus_active(w, cx);
            })
            .unwrap();
        visual.run_until_parked();
        window
            .update(visual, |pane, w, cx| {
                let editor = pane.active_editor().unwrap();
                assert!(editor.focus_handle(cx).contains_focused(w, cx));
                if path.extension().unwrap() == "txt" {
                    assert!(editor.read(cx).input.focus_handle(cx).is_focused(w));
                } else {
                    assert!(editor.focus_handle(cx).is_focused(w));
                }
            })
            .unwrap();
    }
    // Closing a cached preview must not leave focus in its hidden editor.
    window
        .update(visual, |pane, _, cx| pane.close_active(cx))
        .unwrap();
    visual.run_until_parked();
    window
        .update(visual, |pane, w, cx| {
            let editor = pane.active_editor().unwrap();
            assert!(editor.focus_handle(cx).contains_focused(w, cx));
            assert!(editor.read(cx).input.focus_handle(cx).is_focused(w));
        })
        .unwrap();
}

#[gpui::test]
fn entering_panes_defer_file_reads_and_diff_construction(cx: &mut gpui::TestAppContext) {
    let repo = Repo::new();
    let cwd = repo.cwd();
    let git = cx.new(|cx| ProjectGit::new(cwd.clone(), cx));
    let pane = cx.new(|cx| ReviewPane::new(git, Default::default(), cx));
    pane.update(cx, |pane, cx| {
        pane.set_deferred(true, cx);
        pane.open(
            ReviewTab::Changes {
                cwd: cwd.clone(),
                path: None,
                staged: None,
            },
            cx,
        );
        pane.set_visible(true, cx);
    });
    cx.run_until_parked();
    pane.read_with(cx, |pane, _| {
        assert!(!pane.sources[0].loading);
        assert!(pane.sources[0].files.is_none());
        assert!(pane.sources[0].loaded.is_empty());
    });
    pane.update(cx, |pane, cx| pane.set_deferred(false, cx));
    cx.run_until_parked();
    pane.read_with(cx, |pane, _| assert!(pane.sources[0].complete));
    pane.update(cx, |pane, cx| {
        pane.set_deferred(true, cx);
        pane.open(
            ReviewTab::File {
                cwd,
                path: repo.0.join("a.txt").to_string_lossy().into_owned(),
            },
            cx,
        );
    });
    cx.run_until_parked();
    let input = pane.read_with(cx, |pane, cx| {
        pane.active_editor().unwrap().read(cx).input.clone()
    });
    assert!(input.read_with(cx, |input, _| input.content.is_empty()));
    pane.update(cx, |pane, cx| pane.set_deferred(false, cx));
    cx.run_until_parked();
    assert_eq!(
        input.read_with(cx, |input, _| input.content.to_string()),
        "edit1\n"
    );
}

#[gpui::test]
fn reopening_diffs_reuses_views_and_only_reads_changed_files(cx: &mut gpui::TestAppContext) {
    let repo = Repo::new();
    let cwd = repo.cwd();
    let git = cx.new(|cx| ProjectGit::new(cwd.clone(), cx));
    cx.run_until_parked();
    let tab = ReviewTab::Changes {
        cwd: cwd.clone(),
        path: None,
        staged: None,
    };
    let pane = cx.new(|cx| ReviewPane::new(git.clone(), Default::default(), cx));
    pane.update(cx, |pane, cx| {
        pane.open(tab.clone(), cx);
        pane.set_visible(true, cx);
    });
    cx.run_until_parked();
    let (view, a, b) = pane.read_with(cx, |pane, _| {
        let source = &pane.sources[0];
        assert!(source.complete);
        (
            source.view.clone(),
            source.loaded["unstaged:a.txt"].clone(),
            source.loaded["unstaged:b.txt"].clone(),
        )
    });
    pane.update(cx, |pane, cx| {
        pane.set_visible(false, cx);
        pane.set_visible(true, cx);
    });
    cx.run_until_parked();
    pane.read_with(cx, |pane, _| {
        assert!(Arc::ptr_eq(&a, &pane.sources[0].loaded["unstaged:a.txt"]));
        assert!(Arc::ptr_eq(&b, &pane.sources[0].loaded["unstaged:b.txt"]));
    });
    pane.update(cx, |pane, cx| pane.cache_closed(cx));
    drop(pane);
    let reopened = cx.new(|cx| ReviewPane::new(git.clone(), Default::default(), cx));
    reopened.update(cx, |pane, cx| {
        pane.open(tab.clone(), cx);
        pane.set_visible(true, cx);
    });
    cx.run_until_parked();
    reopened.read_with(cx, |pane, _| {
        assert_eq!(
            view, pane.sources[0].view,
            "viewport and highlighting survive closing"
        );
        assert!(Arc::ptr_eq(&a, &pane.sources[0].loaded["unstaged:a.txt"]));
        assert!(Arc::ptr_eq(&b, &pane.sources[0].loaded["unstaged:b.txt"]));
    });
    reopened.update(cx, |pane, cx| pane.cache_closed(cx));
    drop(reopened);
    std::fs::write(repo.0.join("a.txt"), "edit2\n").unwrap();
    std::fs::write(repo.0.join("new.txt"), "new\n").unwrap();
    let reopened = cx.new(|cx| ReviewPane::new(git, Default::default(), cx));
    reopened.update(cx, |pane, cx| {
        pane.open(tab, cx);
        pane.set_visible(true, cx);
    });
    cx.run_until_parked();
    reopened.read_with(cx, |pane, _| {
        let source = &pane.sources[0];
        assert!(!Arc::ptr_eq(&a, &source.loaded["unstaged:a.txt"]));
        assert_eq!(source.loaded["unstaged:a.txt"].diff.current, "edit2\n");
        assert!(Arc::ptr_eq(&b, &source.loaded["unstaged:b.txt"]));
        assert!(source.loaded.contains_key("unstaged:new.txt"));
    });
}

#[gpui::test]
fn closed_diff_cache_is_scoped_and_bounded(cx: &mut gpui::TestAppContext) {
    let git = cx.new(|cx| ProjectGit::new("~".into(), cx));
    let pane = cx.new(|cx| ReviewPane::new(git.clone(), Default::default(), cx));
    let tab = |sha: String| ReviewTab::Commit {
        cwd: "/r".into(),
        short_sha: sha.clone(),
        sha,
        subject: "Test".into(),
    };
    pane.update(cx, |pane, cx| {
        for i in 0..=MAX_CACHED_SOURCES {
            let mut source = pane.source(tab(i.to_string()), cx);
            source.files = Some(Vec::new());
            git.update(cx, |git, _| git.review_cache.remember(&source));
        }
        git.update(cx, |git, _| {
            assert_eq!(git.review_cache.sources.len(), MAX_CACHED_SOURCES);
            assert!(git.review_cache.take(&tab("0".into())).is_none());
            assert!(git.review_cache.take(&tab("1".into())).is_some());
            assert!(!ReviewCache::matches(
                &ReviewTab::Changes {
                    cwd: "/r".into(),
                    path: None,
                    staged: Some(true)
                },
                &ReviewTab::Changes {
                    cwd: "/r".into(),
                    path: None,
                    staged: Some(false)
                }
            ));
            assert!(!ReviewCache::matches(
                &tab("2".into()),
                &ReviewTab::Commit {
                    cwd: "/other-worktree".into(),
                    sha: "2".into(),
                    short_sha: "2".into(),
                    subject: "Test".into()
                }
            ));
        });
        let mut source = pane.source(tab("huge".into()), cx);
        source.files = Some(Vec::new());
        source.loaded.insert(
            "huge".into(),
            Arc::new(Loaded {
                version: None,
                diff: Arc::new(FileDiff {
                    current: "x".repeat(MAX_CACHED_TEXT_BYTES + 1),
                    ..Default::default()
                }),
                unified: None,
                chunks: Arc::default(),
                additions: 0,
                deletions: 0,
                error: None,
            }),
        );
        git.update(cx, |git, _| {
            git.review_cache.remember(&source);
            assert!(git.review_cache.take(&source.tab).is_none());
        });
    });
}
#[gpui::test]
fn file_reviews_are_editable_and_view_switches_preserve_the_unsaved_document(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::EntityInputHandler;
    let repo = Repo::new();
    cx.update(|cx| cx.set_global(crate::settings::Preferences::default()));
    let cwd = repo.cwd();
    let path = repo.0.join("a.txt").to_string_lossy().into_owned();
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            let git = cx.new(|cx| ProjectGit::new(cwd.clone(), cx));
            cx.new(|cx| {
                let mut pane = ReviewPane::new(git, Default::default(), cx);
                pane.open(
                    ReviewTab::File {
                        cwd: cwd.clone(),
                        path: path.clone(),
                    },
                    cx,
                );
                pane.set_visible(true, cx);
                pane
            })
        })
        .unwrap()
    });
    cx.run_until_parked();
    let editor = window
        .update(cx, |pane, w, cx| {
            assert!(pane.sources[0].editor_mode);
            let editor = pane.active_editor().unwrap();
            editor
                .read(cx)
                .input
                .clone()
                .update(cx, |i, cx| i.replace_text_in_range(None, "draft ", w, cx));
            editor
        })
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        cx.update_global::<crate::settings::Preferences, _>(|p, _| p.diff_view = "unified".into())
    });
    cx.run_until_parked();
    window
        .update(cx, |pane, _, cx| {
            assert!(pane.sources[0].editor_mode);
            assert!(editor.read(cx).dirty);
        })
        .unwrap();
    cx.update(|cx| {
        cx.update_global::<crate::settings::Preferences, _>(|p, _| p.diff_view = "split".into())
    });
    cx.run_until_parked();
    window
        .update(cx, |pane, _, cx| {
            assert!(pane.sources[0].editor_mode);
            assert_eq!(pane.active_editor().unwrap(), editor);
            assert!(editor.read(cx).input.read(cx).content.contains("draft"));
            pane.open(
                ReviewTab::Changes {
                    cwd,
                    path: Some(repo.0.join("b.txt").to_string_lossy().into_owned()),
                    staged: Some(false),
                },
                cx,
            );
            assert_eq!(pane.sources.len(), 2);
            assert!(!pane.sources[1].editor_mode);
            assert!(pane.sources[1].editor.is_none());
            assert!(editor.read(cx).dirty);
        })
        .unwrap();
}
fn file(relative: &str, staged: bool, unstaged: bool) -> ChangedFile {
    ChangedFile {
        path: format!("/r/{relative}"),
        relative: relative.into(),
        status: "modified".into(),
        additions: 1,
        deletions: 0,
        staged,
        unstaged,
    }
}
#[test]
fn entries_put_staged_first_and_scope_to_one_side() {
    let files = [file("a", true, true), file("b", false, true)];
    let ids: Vec<_> = entries(&files, None).into_iter().map(|e| e.0).collect();
    assert_eq!(ids, ["staged:a", "unstaged:a", "unstaged:b"]);
    assert_eq!(entries(&files, Some(true)).len(), 1);
    let changes = ReviewTab::Changes {
        cwd: "/r".into(),
        path: None,
        staged: Some(true),
    };
    assert_eq!(changes.label(), "Staged Changes");
    assert!(!changes.same_tab(&ReviewTab::Changes {
        cwd: "/r".into(),
        path: Some("a".into()),
        staged: None
    }));
}
