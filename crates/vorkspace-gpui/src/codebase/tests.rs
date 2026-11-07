use super::*;
use gpui::{HeadlessAppContext, TestAppContext, VisualTestContext, size};
use std::sync::Mutex;
use vorkspace_core::store::Store;

struct Surface(Entity<Page>);
impl Render for Surface {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(pane_fill())
            .font_family(vorkspace_ui::fonts::UI)
            .child(self.0.clone())
    }
}
fn fixture(width: f32, cx: &mut Context<Page>) -> Page {
    let mut page = Page::new("/work/platform".into(), "/work/platform".into(), cx);
    let store = Arc::new(Mutex::new(
        Store::open(std::path::Path::new(":memory:")).unwrap(),
    ));
    page.service = Ok(
        Service::new(store, PathBuf::from("/unused-codebase-fixture"))
            .unwrap()
            .0,
    );
    page.width = width;
    page.snapshot = Snapshot {
        enabled: true,
        phase: "ready".into(),
        root: "/work/platform".into(),
        error: None,
        index: Some(json!({"nodes":2480,"edges":7312,"watch":{"watched":true}})),
    };
    page.symbols = [
        "prepare_release",
        "validate_artifacts",
        "publish_release",
        "read_manifest",
        "verify_signature",
        "create_bundle",
    ]
    .iter()
    .map(|name| Symbol {
        name: (*name).into(),
        qualified: format!("platform.release.{name}"),
        kind: "Function".into(),
        file: "src/release.rs".into(),
        lines: "42–49".into(),
    })
    .collect();
    page.selected = page.symbols.first().cloned();
    page.show_detail = true;
    page.source = Some(cx.new(|cx| Document::prose("```rust\npub fn prepare_release(plan: &ReleasePlan) -> Result<Bundle> {\n    let manifest = read_manifest(&plan.root)?;\n    validate_artifacts(&manifest)?;\n    verify_signature(&manifest)?;\n    create_bundle(plan, manifest)\n}\n```", "", cx)));
    page.relationships = vec![
        ("platform.release.publish_release".into(), true),
        ("platform.release.read_manifest".into(), false),
        ("platform.release.validate_artifacts".into(), false),
        ("platform.release.verify_signature".into(), false),
        ("platform.release.create_bundle".into(), false),
    ];
    page
}

#[test]
fn grouped_symbols_and_relationships_keep_qualified_identity() {
    let rows = symbols(
        &json!({"groups":[{"qn_prefix":"project.module","file":"src/lib.rs","rows":[["run","Function","4-8",1,0]]}]}),
    );
    assert_eq!(rows[0].qualified, "project.module.run");
    let links =
        links_list(&json!({"callers":{"groups":[{"qn_prefix":"project","rows":[["main",1]]}]}}));
    assert_eq!(links, vec![("project.main".into(), true)]);
    let source = graph("quote\"<test>", &links);
    assert!(source.contains("&quot;&lt;test&gt;"));
    assert!(source.contains("n1 --> n0"));
}

#[gpui::test]
fn narrow_page_returns_to_symbols_without_losing_selection(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(gpui::WindowOptions::default(), |_, cx| {
            cx.new(|cx| Surface(cx.new(|cx| fixture(420., cx))))
        })
        .unwrap()
    });
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(size(px(420.), px(780.)));
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("codebase-symbols").is_none());
    let back = visual.debug_bounds("codebase-back").unwrap();
    visual.simulate_click(back.center(), Default::default());
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("codebase-symbols").is_some());
    assert!(visual.debug_bounds("codebase-detail").is_none());
    window
        .update(visual, |surface, _, cx| {
            assert_eq!(
                surface.0.read(cx).selected.as_ref().unwrap().name,
                "prepare_release"
            )
        })
        .unwrap();
}

#[test]
#[ignore = "Requires Metal and VORKSPACE_PAGES_SCREEN_DIR"]
fn native_codebase_capture() {
    let output = PathBuf::from(std::env::var_os("VORKSPACE_PAGES_SCREEN_DIR").unwrap());
    assert!(output.is_dir());
    let mut cx = HeadlessAppContext::with_platform(
        Arc::new(gpui_apple::AppleTextSystem::new()),
        Arc::new(crate::assets::Assets),
        || {
            Ok(Some(Box::new(
                gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
            )))
        },
    );
    for theme in ["dark", "light"] {
        cx.update(|_| {
            configure(
                &vorkspace_ui::preferences::Preferences {
                    theme: theme.into(),
                    ..Default::default()
                },
                false,
                true,
            )
        });
        for width in [1200., 420.] {
            let window = cx
                .open_window(size(px(width), px(780.)), |_, cx| {
                    cx.new(|cx| Surface(cx.new(|cx| fixture(width, cx))))
                })
                .unwrap();
            vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(output.join(format!("codebase-{theme}-{width:.0}.png")))
                .unwrap();
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
        }
    }
}
