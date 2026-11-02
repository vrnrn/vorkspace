//! Each title-tab identity owns one shell independently of its cwd and the dock.
use super::*;
#[cfg(test)]
use vorkspace_core::files::path_key;

impl Shell {
    pub(super) fn terminal_title_tab(
        &self,
        id: u64,
        cwd: &str,
        width: f32,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let tab = TitleTab::Terminal {
            id,
            cwd: cwd.into(),
        };
        let active = self.active_title_tab(cx).as_ref() == Some(&tab);
        let identity = format!("terminal-title-tab-{id}");
        let close_identity = format!("close-terminal-title-tab-{id}");
        let pill = format!("tool:{identity}");
        let display_label = format!("Terminal · {}", project_name(cwd));
        let close_tab = tab.clone();
        let drag_tab = tab.clone();
        let menu_tab = tab.clone();
        let middle_tab = tab.clone();
        let close = active || width >= TAB_CLOSE_MIN_WIDTH;
        div()
            .id(SharedString::from(identity.clone()))
            .debug_selector(move || identity.clone())
            .group("terminal-title-tab")
            .role(Role::Tab)
            .aria_label("Terminal")
            .aria_description(cwd.to_owned())
            .aria_selected(active)
            .cursor(CursorStyle::Arrow)
            .w(px(width))
            .h(px(TITLE_BAR_HEIGHT))
            .flex_shrink_0()
            .flex()
            .items_center()
            .relative()
            .child(
                title_tab_plate(active, false)
                    .relative()
                    .child(self.tab_liquid.row(pill, active))
                    .text_color(content(if active { 1. } else { 0.5 }))
                    .w_full()
                    .min_w_0()
                    .when(close && width >= TAB_CLOSE_MIN_WIDTH, |d| d.pr_7())
                    .when(width < 30., |d| d.px(px(((width - 14.) / 2.).max(0.))))
                    .gap(px(6.))
                    .group_hover("terminal-title-tab", |s| s.text_color(content(1.)))
                    .child(icon("terminal", 14.))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_size(px(TITLE_TAB_TEXT_SIZE))
                            .child(display_label),
                    ),
            )
            .when(close, |d| {
                d.child(
                    div()
                        .id(SharedString::from(close_identity.clone()))
                        .debug_selector(move || close_identity.clone())
                        .role(Role::Button)
                        .aria_label("Close Terminal")
                        .absolute()
                        .right_1()
                        .top(px(10. + TAB_LABEL_DROP))
                        .size(px(20.))
                        .rounded_sm()
                        .flex()
                        .items_center()
                        .justify_center()
                        .opacity(0.)
                        .group_hover("terminal-title-tab", |s| s.opacity(1.))
                        .hover(|s| s.bg(content(0.1)))
                        .child(icon("close", 12.))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.close_title_tabs(
                                std::slice::from_ref(&close_tab),
                                None,
                                window,
                                cx,
                            );
                        })),
                )
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event, _, cx| {
                    this.start_tab_drag(drag_tab.clone(), event, cx)
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.open_menu(
                        MenuTarget::TerminalTitleTab(menu_tab.clone()),
                        Anchor::Point(event.position),
                        window,
                        cx,
                    )
                }),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.close_title_tabs(std::slice::from_ref(&middle_tab), None, window, cx);
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if std::mem::take(&mut this.skip_tab_click) {
                    return;
                }
                this.open_title_tab(&tab, window, cx);
            }))
    }
    /// Every sidebar admission creates a fresh identity. Existing title-tab
    /// selection goes directly to `show_terminal_tab` with that tab's identity.
    pub(super) fn open_terminal_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self
            .workspace
            .new_terminal_tab(self.working_copy().to_owned());
        self.open_title_tab(&tab, window, cx);
    }

    pub(super) fn show_terminal_tab(
        &mut self,
        id: u64,
        cwd: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cwd.starts_with("remote://") {
            self.error = Some("Terminals are not available on remote projects.".into());
            cx.notify();
            return;
        }
        if !cfg!(test)
            && !self.terminal_tabs.contains_key(&id)
            && !std::path::Path::new(&cwd).is_dir()
        {
            self.error = Some(format!(
                "Could not open terminal: directory is unavailable: {cwd}"
            ));
            cx.notify();
            return;
        }
        self.prepare_app_view(window, cx);
        self.leave_app_views(cx);
        let terminal = self
            .terminal_tabs
            .entry(id)
            .or_insert_with(|| {
                // No Shell observation: grid output belongs to this entity, and
                // the title tab has a stable label. Hidden output cannot wake chat.
                cx.new(|cx| Terminal::new(crate::terminal::usable_cwd(cwd.clone()), cx))
            })
            .clone();
        terminal.update(cx, |terminal, _| terminal.visible = true);
        self.active_terminal_tab = Some(id);
        self.remember_title_tab(TitleTab::Terminal { id, cwd });
        self.reveal_active_tab(cx);
        self.update_chat_visibility(cx);
        self.sync_right_panel(cx);
        self.invalidate_chrome(cx);
        self.focus_app_view(window, cx);
        self.persist(cx);
        cx.notify();
    }

    /// Batch closes use the same warning as dock closes. Capture weak entities
    /// so awaiting a prompt neither retains a closed PTY nor closes a successor
    /// created later, including at the same directory or saved tab identity.
    pub(super) fn confirm_terminal_tab_close(
        &mut self,
        tabs: &[TitleTab],
        preferred: Option<TitleTab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let terminals: Vec<_> = tabs
            .iter()
            .filter_map(|tab| {
                let TitleTab::Terminal { id, cwd } = tab else {
                    return None;
                };
                let terminal = self.terminal_tabs.get(id);
                Some((
                    *id,
                    cwd.clone(),
                    terminal.map(Entity::downgrade),
                    terminal.and_then(|terminal| terminal.read(cx).foreground_process()),
                ))
            })
            .collect();
        let running: Vec<_> = terminals
            .iter()
            .filter_map(|(_, cwd, _, process)| {
                process
                    .as_ref()
                    .map(|process| format!("• {} ({process})", project_name(cwd)))
            })
            .collect();
        if running.is_empty() {
            return false;
        }
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            &format!(
                "These terminals are still running:\n{}\n\nClose them anyway?",
                running.join("\n")
            ),
            None,
            &["Close", "Cancel"],
            cx,
        );
        let tabs = tabs.to_vec();
        cx.spawn_in(window, async move |this, cx| {
            if matches!(answer.await, Ok(0)) {
                let _ = this.update_in(cx, |this, window, cx| {
                    let closing: Vec<_> = tabs
                        .into_iter()
                        .filter(|tab| {
                            let TitleTab::Terminal { id, .. } = tab else {
                                return true;
                            };
                            terminals
                                .iter()
                                .find(|(expected_id, _, _, _)| expected_id == id)
                                .is_some_and(|(_, _, expected, _)| match expected {
                                    Some(expected) => {
                                        expected.upgrade().as_ref() == this.terminal_tabs.get(id)
                                            && this.terminal_tabs.contains_key(id)
                                    }
                                    None => !this.terminal_tabs.contains_key(id),
                                })
                        })
                        .collect();
                    this.close_title_tabs_confirmed(&closing, preferred, window, cx);
                });
            }
        })
        .detach();
        true
    }
}

#[cfg(test)]
mod tests;

#[test]
#[ignore = "Requires Metal and VOKKY_TERMINAL_TAB_CAPTURE_PATH"]
fn native_terminal_tab_capture() {
    let output =
        std::path::PathBuf::from(std::env::var_os("VOKKY_TERMINAL_TAB_CAPTURE_PATH").unwrap());
    assert!(output.is_absolute());
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("demo");
    std::fs::create_dir(&project).unwrap();
    let cwd = project.to_string_lossy().into_owned();
    let store = Arc::new(Mutex::new(
        vorkspace_core::store::Store::open(&directory.path().join("fixture.sqlite")).unwrap(),
    ));
    let mut workspace = Workspace::default();
    workspace.add_project(&cwd);
    workspace.project = cwd.clone();
    let tab = workspace.new_terminal_tab(cwd.clone());
    let TitleTab::Terminal { id, .. } = &tab else {
        unreachable!()
    };
    let id = *id;
    workspace.title_order.push(tab.clone());
    workspace
        .scope_focus
        .insert(format!("{}\0{}", path_key(&cwd), path_key(&cwd)), tab);
    let saved = serde_json::to_string(&workspace).unwrap();
    let mut cx = gpui::HeadlessAppContext::with_platform(
        Arc::new(gpui_apple::AppleTextSystem::new()),
        Arc::new(crate::assets::Assets),
        || {
            Ok(Some(Box::new(
                gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
            )))
        },
    );
    vorkspace_ui::clock::use_test_clock(&cx);
    let window = cx
        .open_window(gpui::size(px(1200.), px(800.)), |_, cx| {
            cx.new(|cx| Shell::new(store, Vec::new(), cwd.clone(), Some(saved), cx))
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |view, _, cx| {
        view.downcast::<Shell>().unwrap().update(cx, |shell, cx| {
            let terminal = &shell.terminal_tabs[&id];
            terminal.update(cx, |terminal, cx| {
                terminal.fixture_output(
                    "demo % printf 'Terminal tab ready\\n'\r\nTerminal tab ready\r\n\r\ndemo % ",
                    cx,
                )
            });
        });
    })
    .unwrap();
    cx.run_until_parked();
    vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
    cx.capture_screenshot(window.into())
        .unwrap()
        .save(output)
        .unwrap();
    cx.update_window(window.into(), |_, window, _| window.remove_window())
        .unwrap();
}
