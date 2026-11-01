
use super::*;
fn stage(original: &str, current: &str, chunk: usize) -> Option<String> {
    let file = build_unified_file(original, current, 3);
    let range = *file.chunks.get(chunk)?;
    Some(stage_chunk(original, current, range))
}
fn large_edited_file() -> (String, String) {
    let original: String = (0..50_000).map(|i| format!("line {i}\n")).collect();
    let mut current_lines: Vec<String> = (0..50_000).map(|i| format!("line {i}")).collect();
    for i in (0..50_000).step_by(5_000) {
        current_lines[i] = format!("edited {i}");
    }
    (original, current_lines.join("\n") + "\n")
}
fn changed(file: &UnifiedFile) -> Vec<(LineKind, &str, Option<u32>, Option<u32>)> {
    file.lines
        .iter()
        .filter(|l| l.kind != LineKind::Context)
        .map(|l| (l.kind, l.text.as_str(), l.old_number, l.new_number))
        .collect()
}
fn context(from: u32, to: u32) -> Vec<Line> {
    (from..=to)
        .map(|n| Line {
            kind: LineKind::Context,
            text: format!("c{n}"),
            old_number: Some(n),
            new_number: Some(n),
            chunk: None,
        })
        .collect()
}

#[test]
fn replacement_is_a_delete_then_an_add() {
    let diff = build_unified_file("alpha\nbeta\ngamma\n", "alpha\nBETA\ngamma\n", 3);
    assert_eq!((diff.additions, diff.deletions), (1, 1));
    assert_eq!(
        changed(&diff),
        [
            (LineKind::Del, "beta", Some(2), None),
            (LineKind::Add, "BETA", None, Some(2))
        ]
    );
}
#[test]
fn new_deleted_and_identical_files() {
    let added = build_unified_file("", "one\ntwo\n", 3);
    assert!(added.additions > 0 && added.deletions == 0);
    assert!(added.lines.iter().all(|l| l.kind == LineKind::Add));
    let deleted = build_unified_file("one\ntwo\n", "", 3);
    assert!(deleted.deletions > 0 && deleted.additions == 0);
    assert!(deleted.lines.iter().all(|l| l.kind == LineKind::Del));
    let same = build_unified_file("same\nfile\n", "same\nfile\n", 3);
    assert_eq!((same.additions, same.deletions), (0, 0));
    assert!(same.lines.iter().all(|l| l.kind == LineKind::Context));
}
#[test]
fn unmodified_runs_fold_outside_the_context() {
    let original = (1..=20)
        .map(|i| format!("line-{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let current = original.replace("line-10", "LINE-10");
    let diff = build_unified_file(&original, &current, 3);
    let folds: Vec<_> = diff
        .blocks
        .iter()
        .filter(|b| matches!(b, Block::Fold { .. }))
        .collect();
    assert_eq!(folds.len(), 2);
    assert_eq!(folds[0].lines()[0].text, "line-1");
    assert_eq!(folds[0].lines().last().unwrap().text, "line-6");
    assert_eq!(folds[1].lines()[0].text, "line-14");
    let tiny = build_unified_file("a\nb\nc\n", "a\nB\nc\n", 3);
    assert!(tiny.blocks.iter().all(|b| matches!(b, Block::Hunk { .. })));
}
#[test]
fn folding_keeps_context_around_changes() {
    let mut lines = context(1, 8);
    lines.push(Line {
        kind: LineKind::Del,
        text: "old".into(),
        old_number: Some(9),
        new_number: None,
        chunk: Some(0),
    });
    lines.push(Line {
        kind: LineKind::Add,
        text: "new".into(),
        old_number: None,
        new_number: Some(9),
        chunk: Some(0),
    });
    lines.extend(context(10, 16));
    let blocks = fold_unified_lines(&lines, 2);
    let kinds: Vec<_> = blocks
        .iter()
        .map(|b| matches!(b, Block::Hunk { .. }))
        .collect();
    assert_eq!(kinds, [false, true, false]);
    let texts: Vec<_> = blocks[1].lines().iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, ["c7", "c8", "old", "new", "c10", "c11"]);
    assert!(matches!(blocks[1], Block::Hunk { chunk: Some(0), .. }));
    assert_eq!(fold_unified_lines(&lines, usize::MAX).len(), 1);
}
#[test]
fn missing_final_newline_shows_as_a_change() {
    let diff = build_unified_file("a\nb", "a\nb\n", 3);
    assert_eq!(
        changed(&diff),
        [
            (LineKind::Del, "b", Some(2), None),
            (LineKind::Add, "b", None, Some(2))
        ]
    );
}
#[test]
fn nearby_short_lines_merge_into_one_chunk() {
    // `}` is shorter than CodeMirror's 3-unit gap, so the two edits
    // around it read as one hunk.
    let diff = build_unified_file("a\n}\nb\n", "A\n}\nB\n", 0);
    assert!(diff
        .lines
        .iter()
        .all(|l| l.chunk == Some(0) || l.kind == LineKind::Context));
    assert_eq!(diff.deletions, 3);
    let apart = build_unified_file("a\nkeep\nb\n", "A\nkeep\nB\n", 0);
    assert_eq!(apart.deletions, 2);
    assert!(apart.lines.iter().any(|l| l.chunk == Some(1)));
}
#[test]
fn staging_one_chunk_leaves_the_others() {
    let original = "one\ntwo\nthree\nfour\nfive\nsix\n";
    let current = "ONE\ntwo\nthree\nfour\nfive\nSIX\n";
    assert_eq!(
        stage(original, current, 0).as_deref(),
        Some("ONE\ntwo\nthree\nfour\nfive\nsix\n")
    );
    assert_eq!(
        stage(original, current, 1).as_deref(),
        Some("one\ntwo\nthree\nfour\nfive\nSIX\n")
    );
    assert_eq!(stage(original, current, 2), None);
    // Insertions, deletions and a new final line without a newline.
    assert_eq!(
        stage("a\nc\n", "a\nb\nc\n", 0).as_deref(),
        Some("a\nb\nc\n")
    );
    assert_eq!(stage("a\nb\nc\n", "a\nc\n", 0).as_deref(), Some("a\nc\n"));
    assert_eq!(stage("a\n", "a\nz", 0).as_deref(), Some("a\nz"));
    assert_eq!(stage("", "x\ny\n", 0).as_deref(), Some("x\ny\n"));
    assert_eq!(stage("é\nb\n", "é\nB\n", 0).as_deref(), Some("é\nB\n"));
}
#[test]
fn comments_quote_the_line() {
    let line = Line {
        kind: LineKind::Del,
        text: "let x = 1;".into(),
        old_number: Some(4),
        new_number: None,
        chunk: Some(0),
    };
    assert_eq!(comment_location("src/a`b.rs", &line), "src/a`b.rs:4");
    assert_eq!(
        format_comment("src/a`b.rs", &line, "  why?\r\n "),
        "Diff comment on `src/a\\`b.rs:4` (deleted line):\n\n> -let x = 1;\n\nwhy?"
    );
    assert_eq!(format_comment("a", &line, "  "), "");
}
#[test]
fn columns_count_tabs_and_wide_characters() {
    assert_eq!(visual_columns("ab\tc"), 5);
    assert_eq!(visual_columns("漢字"), 4);
    assert_eq!(visual_columns("e\u{301}"), 1);
}
#[test]
fn large_files_diff_quickly_with_local_chunks() {
    let (original, current) = large_edited_file();
    let start = Instant::now();
    // The budget's outcome depends on the scheduler; the chunks must not.
    let diff = unified_file(&original, &current, 3, None);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
    assert_eq!((diff.additions, diff.deletions), (10, 10));
    assert_eq!(
        diff.blocks
            .iter()
            .filter(|b| matches!(b, Block::Hunk { .. }))
            .count(),
        10
    );
}
#[test]
fn a_missed_budget_coarsens_the_diff_without_losing_changes() {
    let (original, current) = large_edited_file();
    let expired = Instant::now();
    let diff = unified_file(&original, &current, 3, Some(expired));
    // Only the common prefix and suffix survive: "line 0" to "edited 45000".
    assert_eq!((diff.additions, diff.deletions), (45_001, 45_001));
    assert_eq!(diff.chunks.len(), 1);
    assert_eq!(stage_chunk(&original, &current, diff.chunks[0]), current);
}
#[test]
fn staging_applies_the_chunk_the_view_showed_whatever_the_budget() {
    let (original, current) = large_edited_file();
    let shown = unified_file(&original, &current, 3, None);
    let expected = original.replace("line 25000\n", "edited 25000\n");
    assert_eq!(stage_chunk(&original, &current, shown.chunks[5]), expected);
    let coarse = unified_file(&original, &current, 3, Some(Instant::now()));
    assert_ne!(coarse.chunks.get(5), shown.chunks.get(5));
}
