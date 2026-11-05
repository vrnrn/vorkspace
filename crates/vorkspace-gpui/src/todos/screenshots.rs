use super::*;
use gpui::{HeadlessAppContext, size};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use vorkspace_core::store::Store;

struct Surface(Entity<TodosPage>);

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
fn native_todos_capture() {
    let output = PathBuf::from(std::env::var_os("VORKSPACE_PAGES_SCREEN_DIR").unwrap());
    assert!(output.is_absolute() && output.is_dir());
    let store = Store::open(Path::new(":memory:")).unwrap();
    let board = TodoBoard::Project("/work/fixtures/release-platform".into());
    for (title, status) in [
        ("Review the release checklist", TodoStatus::Todo),
        (
            "Confirm the signed desktop build preserves existing projects, saved conversations, and account preferences during an in-place upgrade",
            TodoStatus::Todo,
        ),
        ("Prepare the internal preview", TodoStatus::InProgress),
        ("Run the macOS smoke tests", TodoStatus::InProgress),
        ("Update the setup guide", TodoStatus::Done),
        ("Check the support links", TodoStatus::Done),
    ] {
        store.add_todo(&board, title, status).unwrap();
    }
    let items = store.list_todos(&board).unwrap();
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
                            let mut page = TodosPage::new(store.clone(), cx);
                            page.choices.push(BoardChoice {
                                board: board.clone(),
                                label: "Release platform".into(),
                            });
                            page.board.update(cx, |view, cx| {
                                // Supersede the initial personal-board load.
                                view.generation += 1;
                                view.board = Some(board.clone());
                                view.items = items.clone();
                                view.loaded = true;
                                cx.notify();
                            });
                            page
                        }))
                    })
                })
                .unwrap();
            vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(output.join(format!("todos-{theme}-{layout}.png")))
                .unwrap();
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
        }
    }
}
