//! The edit entry point for views that use the application's preferences.
use gpui::{App, BorrowAppContext, EntityId, Global};
use std::rc::Rc;
use vorkspace_ui::preferences::Preferences;

type Edit<'a> = Box<dyn FnOnce(&mut Preferences) + 'a>;
type Write = dyn Fn(&mut App, Edit<'_>);

struct Writer {
    owner: EntityId,
    write: Rc<Write>,
}
impl Global for Writer {}

pub(crate) fn install(
    owner: EntityId,
    write: impl Fn(&mut App, Edit<'_>) + 'static,
    cx: &mut App,
) -> bool {
    if cx
        .try_global::<Writer>()
        .is_some_and(|old| old.owner == owner)
    {
        return false;
    }
    cx.set_global(Writer {
        owner,
        write: Rc::new(write),
    });
    true
}

pub(crate) fn remember(cx: &mut App, edit: impl FnOnce(&mut Preferences)) {
    if let Some(write) = cx.try_global::<Writer>().map(|owner| owner.write.clone()) {
        write(cx, Box::new(edit));
    } else if cx.has_global::<Preferences>() {
        cx.update_global::<Preferences, _>(|prefs, _| edit(prefs));
    }
}
