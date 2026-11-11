use super::*;
use gpui::{HeadlessAppContext, size};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};
use vorkspace_core::store::Store;

struct Surface(Entity<SearchView>);

impl Render for Surface {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(pane_fill())
            .font_family(vorkspace_ui::fonts::UI)
            .child(self.0.clone())
    }
}

#[test]
#[ignore = "Requires Metal and VORKSPACE_PAGES_SCREEN_DIR"]
fn native_search_capture() {
    const QUERY: &str = "release";
    const PROJECT: &str = "/work/fixtures/team-platform/archive/desktop-release-platform";
    let output = PathBuf::from(std::env::var_os("VORKSPACE_PAGES_SCREEN_DIR").unwrap());
    assert!(output.is_absolute() && output.is_dir());
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().to_string_lossy().into_owned();
    let store = Store::test(Path::new(":memory:")).unwrap();
    let mut session = store.create("codex", PROJECT).unwrap();
    session.title = "Release preparation and signed desktop package verification".into();
    let message = search::message_hit(
        &session,
        3,
        "assistant",
        "The release checklist includes an in-place upgrade on both supported macOS architectures.",
        QUERY,
        false,
    )
    .unwrap();
    let relative =
        "crates/desktop/src/workflow/publishing/verification/macos/desktop/release_checklist.rs";
    let file = search::content(
        vec![search::project::SearchMatch {
            path: format!("{PROJECT}/{relative}"),
            relative: relative.into(),
            line: 72,
            column: 9,
            preview: "    verify_release_checklist(&package, &current_installation)?;".into(),
        }],
        QUERY,
    );
    let store = Arc::new(Mutex::new(store));
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
                theme == "light",
                true,
            );
        });
        for (layout, width) in [("wide", 1200.), ("narrow", 540.)] {
            let window = cx
                .open_window(size(px(width), px(780.)), |_, cx| {
                    cx.new(|cx| {
                        Surface(cx.new(|cx| {
                            // Listing is confined to an empty temporary
                            // directory. Install completed results directly.
                            let mut view = SearchView::new(
                                cwd.clone(),
                                store.clone(),
                                Sources {
                                    summaries: vec![session.clone()],
                                    projects: vec![PROJECT.into()],
                                    ..Default::default()
                                },
                                cx,
                            );
                            view.input = cx.new(|cx| {
                                let mut input = Input::field(
                                    "Search",
                                    "Search everything…",
                                    page::secondary_ink(),
                                    cx,
                                );
                                input.set_text(QUERY.into(), cx);
                                input
                            });
                            view.query = QUERY.into();
                            view.catalog_hits = search::titles(&view.summaries, QUERY);
                            view.catalog_hits
                                .extend(search::projects(&view.projects, QUERY));
                            view.remote = vec![message.clone()];
                            view.content = file.clone();
                            view.rank(cx);
                            view.active = 1;
                            view
                        }))
                    })
                })
                .unwrap();
            vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(output.join(format!("search-{theme}-{layout}.png")))
                .unwrap();
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
        }
    }
}
