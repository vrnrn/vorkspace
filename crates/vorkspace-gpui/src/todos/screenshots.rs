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
    let prerequisite = store
        .add_todo(&board, "Review the release checklist", TodoStatus::Todo)
        .unwrap();
    let detail = store
        .add_todo_with_details(
            &board,
            "Confirm the signed desktop build preserves saved projects and conversations",
            TodoStatus::Todo,
            "## Upgrade checks\n\nCheck an in-place upgrade on a clean macOS account, then record what changed.\n\n- Saved projects remain available\n- Conversations reopen at the last message\n- Unfinished drafts are preserved\n\n### Expected result\n\nThe signed build should open normally and preserve every existing workspace.",
            &["https://example.com/release-plan.png".into()],
            std::slice::from_ref(&prerequisite.id),
        )
        .unwrap();
    for (title, status) in [
        ("Prepare the internal preview", TodoStatus::InProgress),
        ("Review the smoke test results", TodoStatus::InReview),
        ("Run the macOS smoke tests", TodoStatus::InReview),
        ("Update the setup guide", TodoStatus::Done),
        ("Check the support links", TodoStatus::Done),
    ] {
        store.add_todo_with_details(
            &board, title, status,
            "Review the implementation, check the important edge cases, and share the results with the team. Keep the release focused and the upgrade predictable.",
            &[], std::slice::from_ref(&prerequisite.id),
        ).unwrap();
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
    cx.update(|_| {
        configure(
            &vorkspace_ui::preferences::Preferences {
                theme: "dark".into(),
                ..Default::default()
            },
            false,
            true,
        );
    });
    for (layout, width) in [("wide", 1200.), ("tile420", 420.)] {
        let window = cx
            .open_window(size(px(width), px(900.)), |_, cx| {
                cx.new(|cx| {
                    Surface(cx.new(|cx| {
                        let mut page = TodosPage::new(store.clone(), cx);
                        page.set_choices(
                            vec![BoardChoice {
                                board: board.clone(),
                                label: "Release platform".into(),
                            }],
                            cx,
                        );
                        page.board_for_capture().update(cx, |view, cx| {
                            // Supersede the initial personal-board load.
                            view.generation += 1;
                            view.board = Some(board.clone());
                            view.items = items.clone();
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
            .save(output.join(format!("todos-dark-{layout}-board.png")))
            .unwrap();
        window
            .update(&mut cx, |surface, _, cx| {
                surface.0.update(cx, |page, cx| {
                    page.board_for_capture().update(cx, |view, cx| {
                        view.create_open = true;
                        cx.notify();
                    });
                });
            })
            .unwrap();
        vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
        cx.capture_screenshot(window.into())
            .unwrap()
            .save(output.join(format!("todos-dark-{layout}-create.png")))
            .unwrap();
        window
            .update(&mut cx, |surface, _, cx| {
                surface.0.update(cx, |page, cx| {
                    page.board_for_capture().update(cx, |view, cx| {
                        view.create_open = false;
                        view.open_details(&detail, cx);
                    });
                });
            })
            .unwrap();
        vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
        cx.capture_screenshot(window.into())
            .unwrap()
            .save(output.join(format!("todos-dark-{layout}.png")))
            .unwrap();
        window
            .update(&mut cx, |surface, _, cx| {
                surface.0.update(cx, |page, cx| {
                    page.board_for_capture().update(cx, |view, cx| {
                        view.detail_editing = true;
                        cx.notify();
                    });
                });
            })
            .unwrap();
        vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
        cx.capture_screenshot(window.into())
            .unwrap()
            .save(output.join(format!("todos-dark-{layout}-edit.png")))
            .unwrap();
        cx.update_window(window.into(), |_, window, _| window.remove_window())
            .unwrap();
    }
}
