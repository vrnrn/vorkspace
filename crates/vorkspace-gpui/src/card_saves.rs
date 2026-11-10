use gpui::{App, Global};
use vorkspace_runtime::runtime::card_persistence::CardWriter;

struct CardSaves(CardWriter);
impl Global for CardSaves {}

pub(crate) fn writer(cx: &mut App) -> Result<&CardWriter, String> {
    if cx.try_global::<CardSaves>().is_none() {
        cx.set_global(CardSaves(CardWriter::start()?));
        cx.on_app_quit(|cx| {
            let finished = cx.global::<CardSaves>().0.close();
            // Wait for accepted refreshes within the application's quit budget.
            async move {
                let _ = finished.recv().await;
            }
        })
        .detach();
    }
    Ok(&cx.global::<CardSaves>().0)
}

#[cfg(test)]
pub(crate) fn wait_for_saves_for_test(cx: &App) {
    if let Some(saves) = cx.try_global::<CardSaves>() {
        saves.0.wait_for_saves_for_test();
    }
}

#[cfg(test)]
pub(crate) fn finish_for_test(cx: &App) {
    if let Some(saves) = cx.try_global::<CardSaves>() {
        saves.0.finish_for_test();
    }
}
