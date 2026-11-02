//! Explicit disposable Input fixture; entered before opening any store/helper.
use crate::input::Input;
use gpui::{
    AppContext, Context, Entity, Focusable, Render, Window, WindowOptions, div, prelude::*,
};
use std::{path::PathBuf, time::Duration};
struct Fixture {
    field: Entity<Input>,
}
impl Render for Fixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .p_4()
            .w_full()
            .h_full()
            .bg(gpui::rgb(0x242424))
            .text_color(gpui::white())
            .child("Disposable Vorkspace Input — synthetic text only")
            .child(self.field.clone())
    }
}
pub(crate) fn run(output: PathBuf) {
    assert!(output.is_absolute() && output.parent().is_some_and(|p| p.is_dir()));
    gpui::Application::with_platform(std::rc::Rc::new(gpui_macos::MacPlatform::new(false)))
        .with_assets(crate::assets::Assets)
        .run(move |cx| {
            crate::input::bind(cx);
            let panel = cx
                .open_window(WindowOptions::default(), |window, cx| {
                    let field = cx.new(|cx| {
                        Input::field(
                            "Disposable dictation input",
                            "Synthetic text only",
                            gpui::hsla(0., 0., 1., 0.5),
                            cx,
                        )
                    });
                    field.read(cx).focus_handle(cx).focus(window, cx);
                    cx.new(|_| Fixture { field })
                })
                .unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                for _ in 0..100 {
                    cx.background_executor()
                        .timer(Duration::from_millis(200))
                        .await;
                    let state = panel
                        .update(cx, |view, window, cx| {
                            let field = view.field.read(cx);
                            format!(
                                "{}\nfocus={}\n",
                                field.content,
                                field.focus_handle(cx).is_focused(window)
                            )
                        })
                        .unwrap();
                    std::fs::write(&output, state).unwrap();
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
}
