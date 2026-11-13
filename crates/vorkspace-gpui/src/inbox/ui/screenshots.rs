use super::*;
use gpui::{Entity, HeadlessAppContext, Render, size};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use vorkspace_core::store::Store;

struct Surface(Entity<Inbox>);

impl Render for Surface {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(pane_fill())
            .font_family(vorkspace_ui::fonts::UI)
            .child(self.0.clone())
    }
}

const PROJECT: &str = "/work/fixtures/release-platform";
// A non-HTTP URL selects the initials fallback without a network request.
const AVATAR: &str = "fixture:initials";

fn fixture(provider: &str, narrow_detail: bool, cx: &mut Context<Inbox>) -> Inbox {
    let store = Arc::new(Mutex::new(Store::test(Path::new(":memory:")).unwrap()));
    let mut inbox = Inbox::new_for_test(store, vec![], PathBuf::from(PROJECT), cx);
    inbox.state.source = provider.into();
    inbox.state.seen.seeded = true;
    Rc::make_mut(&mut inbox.snapshot).connections.insert(
        provider.into(),
        model::Status {
            connected: true,
            ..Default::default()
        },
    );

    let tracker = provider == "jira";
    let now = chrono::Utc::now();
    let make_item = |number: i64, title: &str, kind: &str, label: &str, color: &str| Item {
        provider: provider.into(),
        number,
        identifier: if tracker {
            format!("REL-{number}")
        } else {
            String::new()
        },
        title: title.into(),
        kind: kind.into(),
        repo: "platform/desktop".into(),
        project_path: PROJECT.into(),
        team_name: if tracker {
            "Desktop release".into()
        } else {
            String::new()
        },
        state: "open".into(),
        url: format!("https://example.invalid/{provider}/{number}"),
        created_at: (now - chrono::Duration::days(1)).to_rfc3339(),
        updated_at: (now - chrono::Duration::hours(number - 39)).to_rfc3339(),
        labels: vec![model::Label {
            name: label.into(),
            color: color.into(),
        }],
        ..Default::default()
    };
    let mut selected = make_item(
        42,
        match provider {
            "github" => "Verify the signed desktop package before publishing",
            "gitlab" => "Publish the release checklist with the pipeline artifacts",
            _ => "Confirm the desktop upgrade path for the next release",
        },
        if tracker { "issue" } else { "pr" },
        "release",
        "6366f1",
    );
    selected.attention_reason = if tracker {
        "assigned"
    } else {
        "review_requested"
    }
    .into();
    selected.assignees.push(model::Person {
        login: "Morgan Lee".into(),
        avatar_url: AVATAR.into(),
    });
    let key = selected.detail_key();
    inbox.state.seen.mark(&selected);
    Rc::make_mut(&mut inbox.snapshot).items = vec![
        selected,
        make_item(
            43,
            "Confirm the upgrade from the previous stable build",
            "issue",
            "macOS",
            "16a34a",
        ),
        make_item(
            44,
            "Document the rollback steps for support",
            "issue",
            "docs",
            "0891b2",
        ),
        make_item(
            45,
            "Check localized release notes before handoff",
            "issue",
            "quality",
            "d97706",
        ),
    ];
    inbox.selected = Some(key.clone());
    inbox.narrow_detail = narrow_detail;
    inbox.finish_detail(
        inbox.detail.generation,
        key,
        Ok(Details {
            author: "Taylor Chen".into(),
            author_avatar_url: AVATAR.into(),
            base_ref_name: if tracker { String::new() } else { "main".into() },
            head_ref_name: if tracker { String::new() } else { "release/package-checks".into() },
            body: "## Release verification\n\nRun the **smoke tests** against the signed package before the release goes live.\n\n- Install over the current stable release.\n- Confirm that existing workspaces reopen with their settings.\n- Record the build number and attach the test results.".into(),
            ..Default::default()
        }),
        Ok(Thread {
            comments: vec![model::Comment {
                id: "release-review".into(),
                author: "Morgan Lee".into(),
                author_avatar_url: AVATAR.into(),
                created_at: (now - chrono::Duration::hours(2)).to_rfc3339(),
                body: "The signed build opened an existing workspace and retained its preferences. I’ll verify the rollback path next.".into(),
                ..Default::default()
            }],
            ..Default::default()
        }),
        cx,
    );
    inbox
}

#[test]
#[ignore = "Requires Metal and VORKSPACE_PAGES_SCREEN_DIR"]
fn native_inbox_capture() {
    let output = PathBuf::from(std::env::var_os("VORKSPACE_PAGES_SCREEN_DIR").unwrap());
    assert!(output.is_absolute() && output.is_dir());
    let mut cx = HeadlessAppContext::with_platform(
        Arc::new(gpui_apple::AppleTextSystem::new()),
        Arc::new(crate::assets::Assets),
        || {
            Ok(Some(Box::new(
                gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
            )))
        },
    );
    vorkspace_ui::clock::use_test_clock(&cx);
    for theme in ["dark", "light"] {
        cx.update(|_| {
            configure(
                &vorkspace_ui::preferences::Preferences {
                    theme: theme.into(),
                    ..Default::default()
                },
                theme == "light",
            );
        });
        for provider in model::PROVIDERS {
            for (layout, width, detail) in [
                ("wide", 1200., false),
                ("narrow-list", 540., false),
                ("narrow-detail", 540., true),
            ] {
                let window = cx
                    .open_window(size(px(width), px(780.)), |_, cx| {
                        cx.new(|cx| Surface(cx.new(|cx| fixture(provider, detail, cx))))
                    })
                    .unwrap();
                vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(output.join(format!("inbox-{provider}-{theme}-{layout}.png")))
                    .unwrap();
                cx.update_window(window.into(), |_, window, _| window.remove_window())
                    .unwrap();
            }
        }
    }
}
