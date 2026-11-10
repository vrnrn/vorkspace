mod accounts;
mod activity;
mod app_menu;
mod assets;
mod automations;
mod avatar;
mod background_helper;
mod bot_header;
mod bots;
mod card_saves;
mod codebase;
mod color_picker;
mod composer;
mod controls;
mod conversation;
mod dev_reload;
mod diagrams;
mod file_link;
mod file_tools;
mod fixtures;
mod git_status;
mod git_tools;
mod image_lightbox;
mod inbox;
mod input;
mod launch;
mod markdown;
mod meetings;
mod menu_bar;
#[cfg(test)]
mod metal_tests;
mod native_view;
mod notes;
mod notifications;
mod panel_kit;
mod pickers;
mod popover;
mod project_background;
mod project_settings;
mod resource_pressure;
mod response_actions;
mod right_panel;
mod search;
mod session_install;
mod settings;
mod shell;
mod sound;
mod syntax;
mod terminal;
mod todos;
mod transcript;
mod usage_footer;
mod visuals;
mod vox;
mod workspace;

use gpui::{
    App, Bounds, KeyBinding, TitlebarOptions, WindowBackgroundAppearance, WindowBounds,
    WindowOptions, actions, point, prelude::*, px, size,
};
actions!(
    application,
    [Quit, Restart, NewWindow, CloseWindow, NextWindow]
);

#[derive(Clone, Copy)]
enum Exit {
    Quit,
    Restart,
}
impl gpui::Global for Exit {}

fn quit(cx: &mut App) {
    request_exit(Exit::Quit, cx);
}

fn restart(cx: &mut App) {
    request_exit(Exit::Restart, cx);
}

fn request_exit(exit: Exit, cx: &mut App) {
    if cx.has_global::<Exit>() {
        return;
    }
    cx.set_global(exit);
    if matches!(exit, Exit::Quit) {
        background_helper::app_quitting(cx);
    }
    let voice = vox::shutdown(cx);
    let codebase = codebase::service(cx).ok();
    cx.spawn(async move |cx| {
        voice.await;
        cx.background_executor()
            .spawn(async move {
                if let Some(service) = codebase {
                    service.shutdown();
                }
                vorkspace_core::process::shutdown_all();
            })
            .await;
        finish_exit(exit, cx).await;
    })
    .detach();
}

async fn finish_exit(exit: Exit, cx: &mut gpui::AsyncApp) {
    loop {
        let waiting = cx.update(|cx| {
            let barriers = shell::bot_write_barriers(cx);
            if barriers.is_empty() {
                if matches!(exit, Exit::Restart)
                    && let Err(error) = vorkspace_macos::restart::relaunch_self()
                {
                    eprintln!("Could not schedule restart: {error}");
                    cx.remove_global::<Exit>();
                } else {
                    cx.quit();
                }
            }
            barriers
        });
        if waiting.is_empty() {
            return;
        }
        for barrier in waiting {
            let _ = barrier.recv().await;
        }
    }
}

/// Use the final frame before the launch window is shown. AppKit's zoom path
/// animates after creation, which exposes a small window before maximization.
fn launch_bounds(cx: &App) -> WindowBounds {
    let restore = Bounds::centered(None, size(px(1180.), px(800.)), cx);
    WindowBounds::Windowed(
        cx.primary_display()
            .map(|display| display.visible_bounds())
            .unwrap_or(restore),
    )
}

pub(crate) struct WorkspaceWindowStart {
    pub(crate) cwd: String,
    pub(crate) saved: Option<String>,
    pub(crate) layout: Option<u64>,
    pub(crate) bounds: WindowBounds,
}

pub(crate) fn open_workspace_window(
    db: vorkspace_runtime::runtime::Database,
    sessions: Vec<vorkspace_core::store::Session>,
    metadata: vorkspace_core::store::SessionSidebarMetadata,
    start: WorkspaceWindowStart,
    cx: &mut App,
) -> gpui::Result<gpui::WindowHandle<shell::Shell>> {
    cx.open_window(
        WindowOptions {
            window_bounds: Some(start.bounds),
            show: false,
            window_min_size: Some(size(px(820.), px(520.))),
            window_background: WindowBackgroundAppearance::Blurred,
            // GPUI owns the custom titlebar drag region.
            app_owns_titlebar_drag: true,
            titlebar: Some(TitlebarOptions {
                title: Some("vorkspace".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(
                    px(vorkspace_ui::theme::TRAFFIC_LIGHT_MARGIN),
                    px((vorkspace_ui::theme::TITLE_BAR_HEIGHT
                        - vorkspace_ui::theme::TRAFFIC_LIGHT_SIZE)
                        / 2.),
                )),
            }),
            ..Default::default()
        },
        |window, cx| {
            let shell = cx.new(|cx| {
                shell::Shell::new_with_metadata(
                    db,
                    sessions,
                    start.cwd,
                    start.saved,
                    metadata,
                    start.layout,
                    cx,
                )
            });
            shell.update(cx, |shell, cx| {
                shell.initialize_scale(window, cx);
                shell.focus_active(window, cx);
                // AppKit rebuilds the buttons on some titlebar changes.
                cx.observe_window_activation(window, |_, window, _| {
                    vorkspace_ui::traffic_lights::pin(window)
                })
                .detach();
            });
            vorkspace_ui::traffic_lights::pin(window);
            window.activate_window();
            shell
        },
    )
}

fn restore_workspace_windows(
    db: vorkspace_runtime::runtime::Database,
    sessions: Vec<vorkspace_core::store::Session>,
    metadata: vorkspace_core::store::SessionSidebarMetadata,
    mut layouts: shell::WindowLayouts,
    fallback: WorkspaceWindowStart,
    explicit_cwd: bool,
    cx: &mut App,
) -> gpui::Result<()> {
    // Open the last focused layout last so it comes to the front.
    layouts
        .entries
        .sort_by_key(|entry| Some(entry.id) == layouts.focused);
    let mut restored = false;
    for (index, entry) in layouts.entries.into_iter().enumerate() {
        let cwd = if explicit_cwd && Some(entry.id) == layouts.focused {
            fallback.cwd.clone()
        } else {
            serde_json::from_str::<workspace::Workspace>(&entry.workspace)
                .map(|workspace| workspace.project)
                .unwrap_or_else(|_| fallback.cwd.clone())
        };
        let bounds = if index == 0 {
            fallback.bounds
        } else {
            shell::additional_window_bounds(None, index, cx)
        };
        match open_workspace_window(
            db.clone(),
            sessions.clone(),
            metadata.clone(),
            WorkspaceWindowStart {
                cwd,
                saved: Some(entry.workspace.to_string()),
                layout: Some(entry.id),
                bounds,
            },
            cx,
        ) {
            Ok(_) => restored = true,
            Err(error) => eprintln!("Cannot restore workspace window: {error}"),
        }
    }
    if !restored {
        open_workspace_window(db, sessions, metadata, fallback, cx)?;
    }
    Ok(())
}

fn main() {
    #[cfg(debug_assertions)]
    if std::env::var_os("VORKSPACE_VOX_PREVIEW").is_some() {
        vox::preview::run();
        return;
    }
    if let Some(code) = vorkspace_core::remote::ssh_askpass::maybe_run() {
        std::process::exit(code);
    }
    let Some(launch::Startup {
        db,
        sessions,
        metadata,
        layouts,
        saved,
        cwd,
        explicit_cwd,
        diagnostics_path,
    }) = launch::prepare()
    else {
        return;
    };
    let application =
        gpui::Application::with_platform(std::rc::Rc::new(gpui_macos::MacPlatform::new(false)))
            .with_assets(assets::Assets);
    application.on_reopen(app_menu::reopen_workspace);
    application.run(move |cx: &mut App| {
        if let Some(path) = diagnostics_path {
            vorkspace_macos::hang_diagnostics::start(cx, path);
        }
        cx.set_reduce_motion(vorkspace_macos::accessibility::reduce_motion());
        dev_reload::install(cx);
        vorkspace_ui::fluid::init(cx);
        vorkspace_macos::updater::start(cx);
        app_menu::init(cx);
        input::bind(cx);
        image_lightbox::bind(cx);
        composer::composer_completion::bind(cx);
        file_tools::file_editor::bind(cx);
        popover::bind(cx);
        file_tools::explorer::bind(cx);
        git_tools::history::bind(cx);
        git_tools::diff_view::bind(cx);
        terminal::bind(cx);
        let modifier = "cmd";
        cx.bind_keys([
            KeyBinding::new(&format!("{modifier}-q"), Quit, None),
            KeyBinding::new(&format!("{modifier}-alt-r"), Restart, None),
        ]);
        shell::bind_keys(cx);
        usage_footer::bind(cx);
        cx.on_window_closed(|cx, _| {
            if shell::workspace_target(cx).is_none() {
                quit(cx)
            }
        })
        .detach();
        let bounds = launch_bounds(cx);
        restore_workspace_windows(
            db,
            sessions,
            metadata,
            layouts,
            WorkspaceWindowStart {
                cwd,
                saved,
                layout: None,
                bounds,
            },
            explicit_cwd,
            cx,
        )
        .expect("open GPUI window");
        cx.activate(true);
    });
}

#[cfg(test)]
mod native_tests {
    use super::*;
    use crate::{input, transcript::TranscriptView};
    use gpui::{Context, Entity, Render, StyleRefinement, Window, div};
    use gpui::{EntityInputHandler, ListOffset, TestAppContext, point};
    use vorkspace_core::transcript::{Row, Transcript};
    struct Surface {
        transcript: Entity<TranscriptView>,
        input: Entity<input::Input>,
    }
    impl Render for Surface {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .flex()
                .flex_col()
                .size_full()
                .child(
                    div().flex_1().min_h_0().child(
                        self.transcript
                            .clone()
                            .cached(StyleRefinement::default().size_full()),
                    ),
                )
                .child(
                    self.input
                        .clone()
                        .cached(StyleRefinement::default().w_full().h(px(76.))),
                )
        }
    }
    #[gpui::test]
    fn launch_uses_the_visible_display_frame_without_a_deferred_zoom(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let bounds = launch_bounds(cx);
            assert_eq!(
                bounds,
                WindowBounds::Windowed(cx.primary_display().unwrap().visible_bounds())
            );
        });
    }
    #[gpui::test]
    fn long_history_virtualizes_and_typing_does_not_rebuild_it(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(900.), px(700.)),
                    ))),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|cx| {
                        let transcript = cx.new(|_| TranscriptView::new());
                        let mut history = Transcript::default();
                        for _ in 0..10_000 {
                            history.begin("assistant");
                            history.append("A bounded block of text.\nA second line.");
                        }
                        transcript.update(cx, |v, cx| v.patch(0, history.rows, cx));
                        Surface {
                            transcript,
                            input: cx.new(input::Input::new),
                        }
                    })
                },
            )
            .unwrap()
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        let root = window.root(cx).unwrap();
        let transcript = root.read_with(cx, |v, _| v.transcript.clone());
        let (passes, rendered) = transcript.read_with(cx, |t, _| (t.render_passes, t.rendered));
        assert!(
            rendered > 0 && rendered < 100,
            "expected viewport-sized work, rendered {rendered} rows"
        );
        window
            .update(cx, |v, w, cx| {
                v.input.update(cx, |i, cx| {
                    i.replace_text_in_range(None, "Typing 🙂 stays local", w, cx)
                })
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(
            transcript.read_with(cx, |t, _| t.render_passes),
            passes,
            "typing must not rerender the transcript entity"
        );
        window
            .update(cx, |v, _, cx| {
                v.transcript.update(cx, |t, cx| {
                    t.list.pause_following_tail();
                    t.list.scroll_to(ListOffset {
                        item_ix: 5000,
                        offset_in_item: px(7.),
                    });
                    let anchor = t.list.logical_scroll_top();
                    t.patch(
                        9999,
                        vec![Row {
                            role: "assistant".into(),
                            text: "Changed streaming tail".into(),
                            first: true,
                            sent_at: None,
                        }],
                        cx,
                    );
                    assert_eq!(t.list.logical_scroll_top().item_ix, anchor.item_ix);
                    assert_eq!(
                        t.list.logical_scroll_top().offset_in_item,
                        anchor.offset_in_item
                    );
                });
            })
            .unwrap();
    }
}
