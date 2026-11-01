
use super::*;

const LANGS: [(Lang, &str); 16] = [
    (Lang::JavaScript, "a.jsx"),
    (Lang::TypeScript, "a.ts"),
    (Lang::Tsx, "a.tsx"),
    (Lang::Json, "a.json"),
    (Lang::Css, "a.css"),
    (Lang::Html, "a.html"),
    (Lang::Python, "a.py"),
    (Lang::Rust, "a.rs"),
    (Lang::Go, "a.go"),
    (Lang::Bash, "a.sh"),
    (Lang::C, "a.c"),
    (Lang::Cpp, "a.cpp"),
    (Lang::Java, "a.java"),
    (Lang::Yaml, "a.yaml"),
    (Lang::Toml, "a.toml"),
    (Lang::Ruby, "a.rb"),
];

const TSX: &str = r#"import { useState } from "react";
// Counts clicks.
export function Counter({ start }: { start: number }) {
  const [count, setCount] = useState(start);
  const isEven = (n: unknown): n is number => typeof n === "number";
  try {
    load();
  } catch (error: unknown) {
    console.log(`failed ${error}`);
  }
  return <button onClick={() => setCount(count + 1)}>{count}</button>;
}
"#;

fn kind_at<'a>(tokens: &'a [Token], text: &str, needle: &str) -> Option<&'a str> {
    let at = text.find(needle).unwrap();
    tokens
        .iter()
        .find(|t| t.from <= at && at < t.to)
        .map(|t| t.kind.as_ref())
}

/// Adjacent same-kind tokens are equivalent to one longer token.
fn normalized(tokens: &[Token]) -> Vec<(usize, usize, String)> {
    let mut out: Vec<(usize, usize, String)> = Vec::new();
    for t in tokens {
        match out.last_mut() {
            Some(last) if last.1 == t.from && last.2 == t.kind.as_ref() => last.1 = t.to,
            _ => out.push((t.from, t.to, t.kind.to_string())),
        }
    }
    out
}

#[test]
fn every_grammar_and_highlight_query_loads() {
    for (lang, path) in LANGS {
        assert_eq!(language_for_path(path), Some(lang));
        assert!(grammar(lang).is_some(), "{lang:?} query must compile");
    }
    assert!(
        !supports("a.swift"),
        "other languages keep the QuickJS path"
    );
}

#[test]
fn tsx_tokens_use_the_editor_palette_groups() {
    let mut session = Session::new("a.tsx").unwrap();
    let analysis = session.analyze(TSX, None);
    let tokens = &analysis.tokens;
    assert_eq!(
        kind_at(tokens, TSX, "import"),
        Some("var(--syntax-keyword)")
    );
    assert_eq!(
        kind_at(tokens, TSX, "\"react\""),
        Some("var(--syntax-string)")
    );
    assert_eq!(
        kind_at(tokens, TSX, "// Counts"),
        Some("var(--syntax-comment)")
    );
    assert_eq!(
        kind_at(tokens, TSX, "useState(start)"),
        Some("var(--syntax-callable)")
    );
    assert_eq!(kind_at(tokens, TSX, "number }"), Some("var(--syntax-type)"));
    assert_eq!(kind_at(tokens, TSX, "button"), Some("var(--syntax-tag)"));
    assert_eq!(kind_at(tokens, TSX, "1)"), Some("var(--syntax-number)"));
    // Template substitutions are not string-coloured.
    assert_eq!(kind_at(tokens, TSX, "error}`"), None);
    assert!(
        tokens.windows(2).all(|w| w[0].to <= w[1].from),
        "sorted, disjoint"
    );
    // Type predicates and typed catch were Lezer grammar gaps; not errors here.
    assert!(
        analysis.diagnostics.is_empty(),
        "{:?}",
        analysis.diagnostics
    );
}

#[test]
fn incremental_highlighting_matches_a_full_pass() {
    for (path, source) in [
        ("a.tsx", TSX),
        (
            "a.rs",
            "/// Doc.\nfn main() {\n    let s = \"hi\";\n    println!(\"{s}\");\n}\n",
        ),
        (
            "a.py",
            "def f(x):\n    return \"y\" + str(x)  # done\n\nclass A:\n    pass\n",
        ),
    ] {
        let mut session = Session::new(path).unwrap();
        let mut text = source.to_owned();
        session.analyze(&text, None);
        // A fixed pseudo-random walk of inserts and deletes, including
        // ones that open strings and comments over the rest of the file.
        let inserts = ["\"", "/*", "x", "{", "\n", "}", "*/", "'", "#", "( ", "// "];
        let mut seed = 0x2545_f491_u64;
        for step in 0..200 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let mut at = (seed as usize) % (text.len() + 1);
            while !text.is_char_boundary(at) {
                at -= 1;
            }
            if step % 3 == 2 && at < text.len() {
                let mut end = (at + 1 + (seed as usize >> 8) % 4).min(text.len());
                while !text.is_char_boundary(end) {
                    end += 1;
                }
                text.replace_range(at..end, "");
            } else {
                text.insert_str(at, inserts[(seed as usize >> 16) % inserts.len()]);
            }
            let incremental = session.analyze(&text, None).tokens;
            let full = Session::new(path).unwrap().analyze(&text, None).tokens;
            assert_eq!(
                normalized(&incremental),
                normalized(&full),
                "{path} step {step}:\n{text}"
            );
        }
    }
}

#[test]
fn html_highlights_its_script_and_style_bodies() {
    let html = "<div class=\"a\">\n<script>const x = 1;</script>\n<style>a { color: red; }</style>\n</div>\n";
    let tokens = Session::new("a.html").unwrap().analyze(html, None).tokens;
    assert_eq!(kind_at(&tokens, html, "div"), Some("var(--syntax-tag)"));
    assert_eq!(
        kind_at(&tokens, html, "class"),
        Some("var(--syntax-property)")
    );
    assert_eq!(
        kind_at(&tokens, html, "const"),
        Some("var(--syntax-keyword)")
    );
    assert_eq!(
        kind_at(&tokens, html, "color"),
        Some("var(--syntax-property)")
    );
}

#[test]
fn folds_start_on_their_opening_line() {
    let js = "function f() {\n  return [\n    1,\n  ];\n}\n";
    let analysis = Session::new("a.js").unwrap().analyze(js, None);
    let ranges: Vec<_> = analysis.folds.iter().map(|f| &js[f.from..f.to]).collect();
    assert_eq!(ranges, ["\n  return [\n    1,\n  ];\n", "\n    1,\n  "]);

    let py = "def f():\n    a = 1\n    return a\nb = 2\n";
    let analysis = Session::new("a.py").unwrap().analyze(py, None);
    assert_eq!(analysis.folds.len(), 1);
    assert_eq!(
        &py[analysis.folds[0].from..analysis.folds[0].to],
        "\n    a = 1\n    return a"
    );
}

#[test]
fn syntax_errors_are_reported_where_the_parse_fails() {
    let mut session = Session::new("a.ts").unwrap();
    let broken = "const a = {\n  b: 1,\n\nfunction g() {}\n";
    let diagnostics = session.analyze(broken, None).diagnostics;
    assert!(!diagnostics.is_empty());
    assert!(
        diagnostics
            .iter()
            .all(|d| d.severity == "error" && d.from < d.to)
    );
    // Fixing it clears them on the incremental pass.
    let fixed = "const a = {\n  b: 1,\n};\nfunction g() {}\n";
    assert!(session.analyze(fixed, None).diagnostics.is_empty());
    // Rust is highlighted but, as in editorLint.ts, not linted.
    assert!(
        Session::new("a.rs")
            .unwrap()
            .analyze("fn (", None)
            .diagnostics
            .is_empty()
    );
}

#[test]
fn git_changes_match_codemirror_line_chunks() {
    let original = "one\ntwo\nthree\nfour\n";
    let current = "one\nTWO\nthree\nfour\nfive\n";
    let changes = changes(original, current, &[]);
    let summary: Vec<_> = changes
        .iter()
        .map(|c| {
            (
                c.kind.as_str(),
                &current[c.from..c.to],
                c.original.as_str(),
                c.old_line,
                c.deleted,
            )
        })
        .collect();
    assert_eq!(
        summary,
        [("mod", "TWO", "two\n", 2, 1), ("add", "five", "", 5, 1)]
    );

    let deleted = changes_for("a\nb\nc\n", "a\nc\n");
    assert_eq!(
        (deleted.kind.as_str(), deleted.from, deleted.to),
        ("del", 2, 2)
    );
    assert_eq!((deleted.original.as_str(), deleted.deleted), ("b\n", 1));

    // A last line without a newline is covered to the end of the file.
    let tail = changes_for("a\nb", "a\nc");
    assert_eq!((tail.from, tail.to), (2, 3));
}

fn changes_for(original: &str, current: &str) -> Change {
    let mut all = changes(original, current, &[]);
    assert_eq!(all.len(), 1);
    all.remove(0)
}

#[test]
fn changes_carry_the_baselines_tokens() {
    let original = "let a = \"x\";\n";
    let current = "let a = 1;\n";
    let mut session = Session::new("a.js").unwrap();
    let analysis = session.analyze(current, Some(original));
    let change = &analysis.changes[0];
    let string = change
        .tokens
        .iter()
        .find(|t| t.kind == "var(--syntax-string)")
        .unwrap();
    assert_eq!(&change.original[string.from..string.to], "\"x\"");
}
