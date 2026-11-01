
use super::*;
#[test]
fn search_and_replace_share_codemirror_unicode_captures_and_word_boundaries() {
    let source = "🙂 café CAFE caféine\nCAFÉ";
    let found = search(source, "café", "tea", false, true, false).unwrap();
    assert_eq!(found.matches.len(), 2);
    assert_eq!(&source[found.matches[0].from..found.matches[0].to], "café");
    assert_eq!(&source[found.matches[1].from..found.matches[1].to], "CAFÉ");
    let found = search(source, "(caf)(é)", "$2$1", false, true, true).unwrap();
    assert_eq!(found.matches[0].insert, "écaf");
    assert!(search(source, "[", "", false, false, true).unwrap().invalid);
    assert!(
        search(source, "^", "x", false, false, true)
            .unwrap()
            .matches
            .is_empty()
    );
    assert_eq!(
        search("a\\nb", "a\\nb", "$1", true, false, false)
            .unwrap()
            .matches[0]
            .insert,
        "$1"
    );
}
#[test]
fn prettier_and_emmet_keep_cursor_bytes_and_the_frontend_defaults() {
    let source = "const emoji='🙂';const value={x:1}";
    let result = format("a.ts", source, source.find("value").unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(
        result.formatted,
        "const emoji = \"🙂\";\nconst value = { x: 1 };\n"
    );
    assert!(result.formatted[result.cursor..].starts_with("value"));
    for (path, source) in [
        ("a.json", "{\"x\":1}"),
        ("a.css", "a{color:red}"),
        ("a.html", "<div><p>hello</p></div>"),
        ("a.md", "# heading\n\nhello   world"),
    ] {
        assert!(format(path, source, 0).unwrap().is_some(), "{path}");
    }
    assert!(format("a.json", "{bad", 0).unwrap().is_none());
    assert!(format("a.rs", "fn main(){}", 0).unwrap().is_none());
    let result = emmet("a.tsx", ".container>span", 15).unwrap().unwrap();
    assert!(result.formatted.contains("className=\"container\""));
    assert!(result.formatted[result.cursor..].starts_with("</span>"));
}
#[test]
fn completions_snippets_and_folds_use_the_frontend_languages() {
    let options = complete("a.ts", "// 🙂\ncons", "// 🙂\ncons".len(), true).unwrap();
    let keyword = options.iter().find(|o| o.label == "const").unwrap();
    assert_eq!(keyword.from, "// 🙂\n".len());
    assert!(options.iter().all(|o| o.insert.is_char_boundary(o.cursor)));
    let source = "fn main() {\n    let emoji = \"🙂\";\n}\n";
    let analysis = analyze("a.rs", source, None).unwrap();
    assert!(!analysis.folds.is_empty());
    assert!(
        analysis
            .folds
            .iter()
            .all(|f| source.is_char_boundary(f.from) && source.is_char_boundary(f.to))
    );
    assert!(
        !analyze("a.md", "# Heading\n\nparagraph\n\n## Next\ntext", None)
            .unwrap()
            .folds
            .is_empty()
    );
}
#[test]
fn tokens_and_folds_cover_files_past_the_initial_parse_viewport() {
    let source = "function f() {\n  return 1;\n}\n".repeat(200);
    let analysis = analyze("a.ts", &source, None).unwrap();
    let last = analysis.tokens.iter().map(|t| t.to).max().unwrap();
    assert!(
        last > source.len() - 20,
        "tokens stop at {last} of {}",
        source.len()
    );
    assert_eq!(analysis.folds.len(), 200);
}
#[test]
fn large_tsx_analysis_highlights_the_end_of_the_current_and_original_file() {
    let source = &(0..300).map(|i| format!("export function Component{i}() {{\n  return (\n    <div title=\"example\">\n      {{true ? <span>hello</span> : null}}\n    </div>\n  );\n}}\n")).collect::<String>();
    let original = format!("{source}\nexport const previousHighlightTail = \"tail\";\n");
    let analysis = analyze("App.tsx", source, Some(&original)).unwrap();
    let last = analysis
        .tokens
        .iter()
        .map(|token| token.to)
        .max()
        .unwrap_or(0);
    assert!(
        last > source.len() - 1_024,
        "highlighting stops at {last} of {} bytes",
        source.len()
    );
    let tail = analysis
        .changes
        .iter()
        .find(|change| change.original.contains("previousHighlightTail"))
        .unwrap();
    assert!(
        tail.tokens
            .iter()
            .any(|token| token.kind == "var(--syntax-keyword)")
    );
    assert!(
        tail.tokens
            .iter()
            .any(|token| token.kind == "var(--syntax-string)")
    );
    assert!(
        analysis
            .tokens
            .iter()
            .all(|token| source.is_char_boundary(token.from) && source.is_char_boundary(token.to))
    );
    assert!(
        analysis
            .folds
            .iter()
            .any(|fold| fold.to > source.len() - 1_024)
    );
}
#[test]
fn analysis_past_its_budget_reports_the_overrun() {
    let error = analyze_within("a.ts", "const x = 1;\n", None, Duration::ZERO).unwrap_err();
    assert_eq!(error, "analysis exceeded its 0ns budget");
}
#[test]
fn analysis_budget_is_not_charged_for_time_off_the_cpu() {
    let before = thread_cpu_time();
    std::thread::sleep(Duration::from_millis(50));
    assert!(thread_cpu_time() - before < Duration::from_millis(50));
}
#[test]
fn hunk_actions_preserve_eof_newlines_and_selected_line_scope() {
    for (original, current) in [
        ("a\nb\n", "a\nx\nb\n"),
        ("a", "a\nx"),
        ("🙂\nb\n", "🙂\nc\n"),
        ("old\n", ""),
        ("", "new"),
    ] {
        let changes = analyze("a.txt", current, Some(original)).unwrap().changes;
        let at = changes[0].from;
        assert_eq!(
            chunk_action(original, current, at, at..at, false)
                .unwrap()
                .as_deref(),
            Some(original)
        );
        assert_eq!(
            chunk_action(original, current, at, at..at, true)
                .unwrap()
                .as_deref(),
            Some(current)
        );
    }
    let original = "one\ntwo\nthree\n";
    let current = "ONE\nTWO\nthree\n";
    assert_eq!(
        chunk_action(original, current, 0, 0..3, true)
            .unwrap()
            .as_deref(),
        Some("ONE\ntwo\nthree\n")
    );
}
