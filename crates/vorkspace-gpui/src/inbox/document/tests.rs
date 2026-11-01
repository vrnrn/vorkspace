
use super::*;
use gpui::{Render, ScrollHandle, TestAppContext, WindowBounds, WindowHandle, WindowOptions};

struct Fixture {
    document: Entity<InboxDocument>,
    scroll: ScrollHandle,
    width: Pixels,
    limit: Option<Pixels>,
    eager: bool,
}
impl Render for Fixture {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if self.eager {
            self.document.update(cx, |doc, cx| {
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .children((0..doc.rows.len()).map(|ix| doc.row(ix, self.width, window, cx)))
                    .into_any_element()
            })
        } else {
            DocumentElement::new(self.document.clone(), self.width, self.limit).into_any_element()
        };
        div()
            .size_full()
            .id("document-scroll")
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(body)
    }
}
fn fixture(text: String, limit: Option<Pixels>, cx: &mut TestAppContext) -> WindowHandle<Fixture> {
    let window = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(640.), px(320.)),
                ))),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|cx| Fixture {
                    document: cx.new(|cx| InboxDocument::new(text, PathBuf::from("/repo"), cx)),
                    scroll: ScrollHandle::new(),
                    width: px(640.),
                    limit,
                    eager: false,
                })
            },
        )
        .unwrap()
    });
    draw(window, cx);
    window
}
fn draw(window: WindowHandle<Fixture>, cx: &mut TestAppContext) {
    window.update(cx, |_, _, cx| cx.notify()).unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
}
fn prose() -> String {
    (0..300).map(|ix| format!("### Finding {ix}\n\nCheck **retries** in `src/main.rs` before returning from this operation. Preserve the reader's place.\n\n")).collect()
}
#[gpui::test]
fn scroll_keeps_exact_heights_and_only_materializes_visible_markdown(cx: &mut TestAppContext) {
    let window = fixture(prose(), None, cx);
    let (before, height) = window
        .update(cx, |view, _, cx| {
            let doc = view.document.read(cx);
            assert!(doc.rendered.len() < 15);
            assert_eq!(doc.measured, doc.rows.len());
            (doc.measured, view.scroll.max_offset().y)
        })
        .unwrap();
    window
        .update(cx, |view, _, _| {
            view.scroll.set_offset(point(px(0.), px(-6000.)))
        })
        .unwrap();
    draw(window, cx);
    window
        .update(cx, |view, _, cx| {
            let doc = view.document.read(cx);
            assert_eq!(
                doc.measured, before,
                "scrolling must reuse exact row heights"
            );
            assert!(doc.rendered.len() < 15);
            assert!(doc.rendered[0] > 30, "scroll must reach later prose");
            assert_eq!(view.scroll.max_offset().y, height);
            view.eager = true;
        })
        .unwrap();
    draw(window, cx);
    window
        .update(cx, |view, _, _| {
            assert!(
                (view.scroll.max_offset().y - height).abs() < px(1.),
                "flow geometry must match the original eager column: eager {:?}, retained {:?}",
                view.scroll.max_offset().y,
                height
            );
        })
        .unwrap();
}
#[gpui::test]
fn collapsed_body_skips_hidden_layout_and_expands_at_the_same_width(cx: &mut TestAppContext) {
    let window = fixture(prose(), Some(px(180.)), cx);
    window
        .update(cx, |view, _, cx| {
            let doc = view.document.read(cx);
            assert!(doc.measured < 10);
            assert!(doc.measured < doc.rows.len());
            assert_eq!(view.scroll.max_offset().y, px(0.));
            view.limit = None;
        })
        .unwrap();
    draw(window, cx);
    window
        .update(cx, |view, _, cx| {
            let doc = view.document.read(cx);
            assert_eq!(doc.measured, doc.rows.len());
            assert!(view.scroll.max_offset().y > px(1000.));
            view.width = px(320.);
        })
        .unwrap();
    draw(window, cx);
    window
        .update(cx, |view, _, cx| {
            let doc = view.document.read(cx);
            assert_eq!(doc.layout.as_ref().unwrap().width, px(320.));
            assert_eq!(
                doc.measured,
                doc.rows.len() * 2,
                "width changes must invalidate cached heights"
            );
        })
        .unwrap();
}
#[gpui::test]
fn tall_code_fence_keeps_full_scroll_range_but_builds_only_visible_lines(cx: &mut TestAppContext) {
    let code: String = (0..4000)
        .map(|ix| format!("const line_{ix} = {ix};\n"))
        .collect();
    let window = fixture(format!("```ts\n{code}```"), None, cx);
    window
        .update(cx, |view, _, cx| {
            assert!(view.scroll.max_offset().y > px(79000.));
            assert!(view.document.read(cx).code_rendered.get() < 20);
            view.scroll.set_offset(point(px(0.), px(-40000.)));
        })
        .unwrap();
    draw(window, cx);
    window
        .update(cx, |view, _, cx| {
            assert!(view.document.read(cx).code_rendered.get() < 20);
            assert!(view.document.read(cx).code_rendered.get() > 0);
            assert!(view.scroll.offset().y < px(-39000.));
        })
        .unwrap();
}
