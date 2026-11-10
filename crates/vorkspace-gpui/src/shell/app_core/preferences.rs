//! Publish shared preferences once and apply effects at their process owner.
use super::*;
use vorkspace_ui::preferences::Preferences;

impl AppCore {
    pub(in crate::shell) fn initialize_preferences(
        &mut self,
        workspace: &Workspace,
        cx: &mut Context<Self>,
    ) -> Arc<WindowShared> {
        let shared = self
            .layouts
            .shared
            .get_or_insert_with(|| Arc::new(WindowShared::from_workspace(workspace)))
            .clone();
        let owner = cx.weak_entity();
        if crate::preference_writes::install(
            cx.entity_id(),
            move |cx, edit| {
                let _ = owner.update(cx, |core, cx| core.remember_preference(edit, cx));
            },
            cx,
        ) {
            vorkspace_core::cli::initialize(&shared.preferences.binary_paths);
            crate::meetings::capture::init(cx);
            self.publish_preferences(&shared.preferences, true, cx);
        }
        shared
    }

    fn remember_preference(&mut self, edit: impl FnOnce(&mut Preferences), cx: &mut Context<Self>) {
        let Some(shared) = &mut self.layouts.shared else {
            return;
        };
        let mut preferences = shared.preferences.clone();
        edit(&mut preferences);
        if preferences == shared.preferences {
            return;
        }
        Arc::make_mut(shared).preferences = preferences;
        let shared = shared.clone();
        self.stage_layouts(cx);
        self.publish_preferences(&shared.preferences, false, cx);
        cx.notify();
    }

    pub(super) fn publish_preferences(
        &mut self,
        prefs: &Preferences,
        initialize: bool,
        cx: &mut Context<Self>,
    ) {
        let previous = if initialize {
            None
        } else {
            cx.try_global::<Preferences>()
        };
        if previous == Some(prefs) {
            return;
        }
        let previous = previous.cloned();
        let changed = |diff: fn(&Preferences, &Preferences) -> bool| {
            previous.as_ref().is_none_or(|old| diff(old, prefs))
        };
        cx.set_global(prefs.clone());
        if changed(|a, b| a.worktree_setup != b.worktree_setup) {
            vorkspace_core::worktree_setup::set_commands(&prefs.worktree_setup);
        }
        if changed(|a, b| {
            a.clipboard_history != b.clipboard_history
                || a.bot_menu_bar_icon != b.bot_menu_bar_icon
                || a.dictation_enabled != b.dictation_enabled
                || a.dictation_microphone != b.dictation_microphone
                || a.dictation_words != b.dictation_words
                || a.keybindings.get("App: Dictation") != b.keybindings.get("App: Dictation")
        }) {
            crate::background_helper::configure(
                prefs.clipboard_history,
                prefs.bot_menu_bar_icon,
                crate::background_helper::dictation_configuration(prefs),
                cx,
            );
        }
        if changed(|a, b| a.idle_agents != b.idle_agents) {
            vorkspace_core::provider::configure_idle_agents(prefs.idle_agents as usize);
        }
        if changed(|a, b| a.keybindings != b.keybindings) {
            crate::settings::apply_keybindings(prefs, cx);
        }
        if changed(|a, b| a.autosave != b.autosave || a.file_word_wrap != b.file_word_wrap) {
            crate::menu_bar::refresh(prefs, cx);
        }
        if changed(|a, b| {
            a.mask_emails != b.mask_emails || a.show_remaining_usage != b.show_remaining_usage
        }) && let Some(center) = crate::accounts::get(cx)
        {
            center.update(cx, |center, cx| {
                center.mask_emails = prefs.mask_emails;
                center.show_remaining = prefs.show_remaining_usage;
                cx.notify();
            });
        }
        if let Some(receiver) = self.quick_shortcut.configure(prefs) {
            self.quick_shortcut.task = Some(cx.spawn(async move |_, cx| {
                while receiver.recv().await.is_ok() {
                    cx.update(|cx| {
                        if let Some(target) =
                            workspace_target(cx).and_then(|window| window.downcast::<Shell>())
                        {
                            let _ = target.update(cx, Shell::toggle_quick_composer);
                        }
                    });
                }
            }));
        }
    }
}
