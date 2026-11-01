use super::*;
use serde_json::json;
fn call(name: &str, input: Value) -> ToolCall {
    let mut call = ToolCall::new("one", name);
    call.input = input.to_string();
    call
}

#[test]
fn search_pills_separate_simple_regex_alternatives() {
    for (name, input) in [
        (
            "Grep",
            json!({"pattern":"model_settings_menu|render_model_settings|self.models|models.clone"}),
        ),
        (
            "Bash",
            json!({"command":"rg -n 'model_settings_menu|render_model_settings|self.models|models.clone' src"}),
        ),
    ] {
        let shown = call(name, input).presentation();
        assert_eq!(
            shown.search_patterns(),
            [
                "model_settings_menu",
                "render_model_settings",
                "self.models",
                "models.clone"
            ]
        );
    }
}

#[test]
fn search_pills_preserve_regex_structure_and_literal_searches() {
    for query in [
        r"foo\|bar",
        "[a|b]",
        "(foo|bar)|baz",
        "(?i)foo|bar",
        "foo||bar",
        "|foo",
        "foo|",
    ] {
        let shown = call("Grep", json!({"pattern":query})).presentation();
        assert_eq!(shown.search_patterns(), [query]);
    }
    for command in [
        "rg -F 'foo|bar' src",
        "rg -nF 'foo|bar' src",
        "rg --fixed-strings 'foo|bar' src",
        "grep 'foo|bar' src/a.rs",
        "fgrep 'foo|bar' src/a.rs",
    ] {
        let shown = call("Bash", json!({"command":command})).presentation();
        assert_eq!(shown.search_patterns(), ["foo|bar"], "{command}");
    }
    let shown = call("WebSearch", json!({"query":"foo|bar"})).presentation();
    assert_eq!(shown.search_patterns(), ["foo|bar"]);
}
#[test]
fn cd_only_moves_later_stages_when_execution_establishes_it() {
    let path = |command: &str| {
        call("Bash", json!({"command": command, "cwd":"/repo"}))
            .presentation_in(Some("/repo"))
            .file_path
    };
    assert_eq!(path("cd sub && cat f.rs").as_deref(), Some("sub/f.rs"));
    // The read happened before the final directory change.
    assert_eq!(
        path("cd sub && cat f.rs && cd -").as_deref(),
        Some("sub/f.rs")
    );
    assert_eq!(
        path("cd sub && cat f.rs && cd").as_deref(),
        Some("sub/f.rs")
    );
    assert_eq!(
        path("cd sub && cat f.rs && cd -- ..").as_deref(),
        Some("sub/f.rs")
    );
    // A pipeline cd runs in a subshell.
    assert_eq!(path("cd sub | cat README.md").as_deref(), Some("README.md"));
    // The directory change may have failed or never run: no trustworthy path.
    for command in [
        "cd missing || cat README.md",
        "cd sub; cat README.md",
        "false || cd sub && cat README.md",
        "cd - && cat README.md",
        "cd && cat README.md",
    ] {
        assert_eq!(path(command), None, "{command}");
    }
    let steps = |command: &str| {
        call("Bash", json!({"command": command, "cwd":"/repo"}))
            .presentation_in(Some("/repo"))
            .steps
            .into_iter()
            .map(|step| step.file_path)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        steps("cd sub && cat f.rs && cd - && npm test"),
        [None, Some("sub/f.rs".into()), None, None]
    );
    assert_eq!(steps("cd sub || cat f.rs && npm test"), [None, None, None]);
}
#[test]
fn relative_file_targets_follow_a_preceding_cd() {
    let single = call(
        "Bash",
        json!({"command":"cd crates && sed -n '1,20p' vorkspace-core/src/lib.rs", "cwd":"/repo"}),
    )
    .presentation_in(Some("/repo"));
    assert_eq!(
        single.file_path.as_deref(),
        Some("crates/vorkspace-core/src/lib.rs")
    );
    let chain = call(
            "Bash",
            json!({"command":"cd crates && sed -n '1,20p' a/b.rs && cd /abs && cat c.rs && cat ./d.rs && npm test", "cwd":"/repo"}),
        )
        .presentation_in(Some("/repo"));
    let paths: Vec<_> = chain
        .steps
        .iter()
        .filter_map(|s| s.file_path.as_deref())
        .collect();
    assert_eq!(paths, ["crates/a/b.rs", "/abs/c.rs", "/abs/d.rs"]);
    let plain = call(
        "Bash",
        json!({"command":"cat src/a.rs && cat src/b.rs && npm test", "cwd":"/repo"}),
    )
    .presentation_in(Some("/repo"));
    let paths: Vec<_> = plain
        .steps
        .iter()
        .filter_map(|s| s.file_path.as_deref())
        .collect();
    assert_eq!(paths, ["src/a.rs", "src/b.rs"]);
}
#[test]
fn live_codex_uses_last_meaningful_stage_and_cwd_boundary() {
    let call = call(
        "commandExecution",
        json!({"command":"cat /repo/src/a.rs | rg needle", "cwd":"/repo", "commandActions":[{"type":"read", "path":"/repo/src/a.rs"},{"type":"search","query":"needle","path":"/repo/src/a.rs"},{"type":"unknown"}]}),
    );
    let shown = call.presentation();
    assert_eq!(
        (shown.action.as_str(), shown.target.as_str()),
        ("Find", "needle")
    );
    assert_eq!(shown.file_path.as_deref(), Some("/repo/src/a.rs"));
    assert_eq!(
        display_path("/repository/file", Some("/repo")),
        "/repository/file"
    );
}
#[test]
fn read_supports_nested_arguments_and_line() {
    let call = call(
        "Read",
        json!({"arguments":{"file_path":"/repo/src/lib.rs","offset":42},"cwd":"/repo"}),
    );
    let shown = call.presentation();
    assert_eq!(shown.target, "src/lib.rs");
    assert_eq!(shown.file_path.as_deref(), Some("/repo/src/lib.rs"));
    assert_eq!(shown.start_line, Some(42));
}
#[test]
fn scripts_are_unwrapped_only_for_display() {
    let input =
        json!({"command":["/bin/zsh","-lc","sed -n '12,30p' 'src/my file.rs'"], "cwd":"/repo"});
    let call = call("commandExecution", input.clone());
    let shown = call.presentation();
    assert_eq!(
        (shown.action.as_str(), shown.target.as_str()),
        ("Read", "src/my file.rs")
    );
    assert_eq!(shown.start_line, Some(12));
    assert_eq!(
        shown.command.as_deref(),
        Some("sed -n '12,30p' 'src/my file.rs'")
    );
    assert_eq!(call.input, input.to_string());
}
#[test]
fn opaque_or_mutating_commands_stay_commands() {
    for command in [
        "npm run cat",
        "python -c 'print(1)'",
        "cat x; cargo test",
        "sed -i '1,5p' file.rs",
        "cat x > y",
        "cat $(touch x)",
        "rg --pre script needle src",
        "head -n 20",
    ] {
        let shown = call("Bash", json!({"command":command})).presentation();
        assert_eq!(shown.action, "Run", "{command}");
        assert_eq!(shown.target, command);
        assert_eq!(shown.file_path, None);
    }
}
#[test]
fn raw_execution_targets_preserve_the_entire_original_command() {
    for command in [
        "cargo test --package vorkspace-core --lib tool::presentation -- --nocapture".into(),
        format!("/private/tmp/{}", "unbrokentoken".repeat(400)),
        "  python -c 'print(\"quoted café 🦀\")\nprint(\"second line 你好\")'\n".into(),
        "/bin/zsh -lc 'python -c \"print(1)\"'".into(),
    ] {
        for name in ["Bash", "commandExecution", "exec_command"] {
            let input = json!({"arguments": json!({"command": command}).to_string()});
            let call = call(name, input.clone());
            let shown = call.presentation_in(Some("/repo"));
            assert_eq!(shown.action, "Run");
            assert_eq!(shown.target, command, "{name}");
            assert_eq!(call.input, input.to_string());
        }
    }
    for empty in ["", " \n"] {
        assert_eq!(
            call("Bash", json!({"command": empty}))
                .presentation()
                .target,
            "Bash"
        );
    }
}
#[test]
fn old_rollout_actions_and_pathless_file_lists_are_readable() {
    let shown = call(
        "commandExecution",
        json!({"command":"rg --files -g AGENTS.md", "parsed_cmd":[{"type":"listFiles"}]}),
    )
    .presentation();
    assert_eq!(
        (shown.action.as_str(), shown.target.as_str()),
        ("Find", "files")
    );
    let shown = call("commandExecution", json!({"command":"opaque", "parsed_cmd":[{"type":"read","path":"/repo/a.rs"}], "cwd":"/repo"})).presentation();
    assert_eq!(shown.target, "a.rs");
}
#[test]
fn encoded_arguments_and_worktree_base_are_display_only() {
    let input = json!({"arguments":"{ \"filePath\": \"/repo/feature/src/main.rs\", \"line\": 9 }"});
    let call = call("Read", input.clone());
    let shown = call.presentation_in(Some("/repo/feature"));
    assert_eq!(shown.target, "src/main.rs");
    assert_eq!(
        shown.file_path.as_deref(),
        Some("/repo/feature/src/main.rs")
    );
    assert_eq!(shown.start_line, Some(9));
    assert_eq!(call.input, input.to_string());
    assert_eq!(
        serde_json::from_str::<Value>(&call.input_detail()).unwrap(),
        input
    );
}
#[test]
fn chained_commands_split_into_captioned_steps() {
    let shown = call(
            "Bash",
            json!({"command":"git status --short && cat package.json && sed -n '1,260p' /repo/scripts/build.mjs; npm test || true", "cwd":"/repo", "description":"Inspect build scripts\nmore"}),
        )
        .presentation();
    assert_eq!(shown.action, "Run");
    assert_eq!(shown.description.as_deref(), Some("Inspect build scripts"));
    let steps: Vec<_> = shown
        .steps
        .iter()
        .map(|s| (s.op, s.action, s.target.as_str()))
        .collect();
    assert_eq!(
        steps,
        [
            (None, "Run", "git status --short"),
            (Some("&&"), "Read", "package.json"),
            (Some("&&"), "Read", "scripts/build.mjs"),
            (Some(";"), "Run", "npm test"),
            (Some("||"), "Run", "true"),
        ]
    );
    assert_eq!(shown.steps[2].start_line, Some(1));
    assert_eq!(
        shown.steps[2].file_path.as_deref(),
        Some("/repo/scripts/build.mjs")
    );
}
#[test]
fn opaque_or_single_commands_have_no_steps() {
    for command in [
        "cargo test",
        "rg foo src | head",
        "echo 'a && b'",
        "cd $(pwd) && ls",
        "(cd x && make) && ls",
        "cat <<EOF && ls\nx\nEOF",
        "cargo build 2>&1 | tail",
    ] {
        let shown = call("Bash", json!({"command":command})).presentation();
        assert!(shown.steps.is_empty(), "{command}");
    }
    let shown = call("Bash", json!({"command":"cd src && cat a.rs"})).presentation();
    assert_eq!(shown.action, "Read");
    assert!(shown.steps.is_empty());
}
