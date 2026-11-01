//! Compact semantic tool captions with clickable file targets.
use super::{TranscriptView, tasks};
use gpui::{Animation, AnimationExt, AnyElement, Context, Div, Role, div, prelude::*, px, rgb};
use vorkspace_core::tool::{CommandStep, ToolCall, ToolState};
use vorkspace_ui::theme::*;

/// Shell plumbing that says nothing about what a chained command was for.
fn is_noise(step: &CommandStep) -> bool {
    step.action == "Run"
        && matches!(
            step.command.split_whitespace().next(),
            Some("cd" | "pwd" | "echo" | "printf" | "true" | "false" | "sleep" | "clear")
        )
}

/// `git status --short` reads as `git status`; flags and paths are detail.
fn command_label(command: &str) -> String {
    let mut words = command.split_whitespace();
    let program = words.next().unwrap_or("");
    let program = program.rsplit('/').next().unwrap_or(program);
    match words.next() {
        Some(sub)
            if !sub.starts_with('-')
                && !sub.contains(['/', '.', '=', '\'', '"'])
                && sub.len() <= 16 =>
        {
            format!("{program} {sub}")
        }
        _ => program.to_owned(),
    }
}

/// A shared fixed box keeps overflow counts aligned with the pattern pills.
fn search_pill() -> Div {
    div()
        .flex()
        .min_w_0()
        .h(px(24.))
        .items_center()
        .px_2()
        .border_1()
        .border_color(content(0.))
        .rounded(px(6.))
        .bg(content(0.06))
}

impl TranscriptView {
    pub(super) fn tool(
        &mut self,
        tool: ToolCall,
        start: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.tool_in(tool, start, None, None, cx)
    }

    pub(super) fn tool_in(
        &mut self,
        tool: ToolCall,
        start: usize,
        scope: Option<&str>,
        width: Option<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(panel) = tasks::render(self, &tool, start, width, cx) {
            return panel;
        }
        let identity = scope.map_or_else(|| start.to_string(), |scope| format!("{start}:{scope}"));
        if tool.is_compaction() {
            let running = tool.state == ToolState::Running;
            let failed = tool.state == ToolState::Failed;
            let label = tool.summary().0;
            return div()
                .id(gpui::SharedString::from(format!("tool:{identity}")))
                .debug_selector(move || format!("tool-{start}"))
                .role(Role::Group)
                .aria_label(label.clone())
                .flex()
                .w_full()
                .min_w_0()
                .items_center()
                .gap_2()
                .py_1()
                .line_height(px(20.))
                .text_color(if failed {
                    rgb(0xf87171).into()
                } else {
                    content(0.55)
                })
                .child(if running {
                    div()
                        .size(px(14.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            vorkspace_ui::session_mark::spinner_dots(0).with_animation(
                                gpui::SharedString::from(format!("compaction-loader:{identity}")),
                                Animation::new(std::time::Duration::from_millis(1_200))
                                    .repeat()
                                    .with_max_fps(30.),
                                |_, phase| {
                                    vorkspace_ui::session_mark::spinner_dots(
                                        (phase * 1_200.) as u64,
                                    )
                                },
                            ),
                        )
                        .into_any_element()
                } else {
                    let glyph = match tool.state {
                        ToolState::Completed => "check",
                        ToolState::Failed => "close",
                        _ => "minus",
                    };
                    icon(glyph, 14.)
                        .text_color(if failed {
                            rgb(0xf87171).into()
                        } else {
                            content(0.45)
                        })
                        .into_any_element()
                })
                .child(div().min_w_0().truncate().child(if running {
                    crate::activity::shimmer(label, self.started_at.elapsed().as_secs_f32())
                } else {
                    div().child(label).into_any_element()
                }))
                .into_any_element();
        }
        let shown = tool.presentation_in(self.file_link_base.as_deref());
        let failed = matches!(tool.state, ToolState::Failed | ToolState::Cancelled);
        let is_search =
            matches!(shown.action.as_str(), "Find" | "Search") && !shown.target.is_empty();
        let action = if is_search { "Search" } else { &shown.action };
        let is_file = matches!(shown.action.as_str(), "Read" | "Edit" | "Write" | "List");
        let target_path = if is_file {
            shown.file_path.as_deref().map(|path| {
                let (path, location) = crate::file_tools::file_editor::file_location(
                    path,
                    self.file_link_base.as_deref().unwrap_or(""),
                );
                if location.is_none() {
                    shown
                        .start_line
                        .map_or(path.clone(), |line| format!("{path}:{line}"))
                } else {
                    location.map_or(path.clone(), |(line, col)| format!("{path}:{line}:{col}"))
                }
            })
        } else {
            None
        };
        let link = |path: &str, line: Option<u64>| {
            let (path, location) = crate::file_tools::file_editor::file_location(
                path,
                self.file_link_base.as_deref().unwrap_or(""),
            );
            match location {
                Some((line, col)) => format!("{path}:{line}:{col}"),
                None => line.map_or(path.clone(), |line| format!("{path}:{line}")),
            }
        };
        let chain = shown.action == "Run" && !shown.steps.is_empty();
        let is_command = shown.action == "Run" && !chain;
        let caption = match (&shown.description, chain) {
            (Some(description), true) => description.clone(),
            _ => format!("{action} {}", shown.target),
        };
        let state_label = match tool.state {
            ToolState::Running => "Running",
            ToolState::Completed => "Completed",
            ToolState::Failed => "Failed",
            ToolState::Cancelled => "Cancelled",
        };
        // Chained commands and failures share one disclosure: steps, then error.
        let error_key = format!("tool-detail:{start}:{}", tool.id);
        let error_open = self.agent_panels.part_open(&error_key, false);
        let has_error = failed && !tool.output.trim().is_empty();
        let has_detail = has_error || chain;
        let chips = if chain {
            let mut seen = std::collections::HashSet::new();
            let chips: Vec<_> = shown
                .steps
                .iter()
                .filter(|step| !is_noise(step))
                .filter_map(|step| {
                    let (label, path) = match step.action {
                        "Read" | "List" => (
                            step.target
                                .rsplit(['/', '\\'])
                                .find(|part| !part.is_empty())
                                .unwrap_or(&step.target)
                                .to_owned(),
                            step.file_path
                                .as_deref()
                                .map(|path| link(path, step.start_line)),
                        ),
                        "Find" => (step.target.clone(), None),
                        _ => (command_label(&step.command), None),
                    };
                    seen.insert((step.action, label.clone()))
                        .then_some((step.action, label, path))
                })
                .collect();
            chips
        } else {
            Vec::new()
        };
        let steps = if chain {
            shown
                .steps
                .iter()
                .map(|step| {
                    let path = step
                        .file_path
                        .as_deref()
                        .map(|path| link(path, step.start_line));
                    (step.op, step.command.clone(), path)
                })
                .collect()
        } else {
            Vec::new()
        };
        let error_detail = tool.output.clone();
        let summary = div()
            .id(gpui::SharedString::from(format!("tool:{identity}")))
            .debug_selector(move || format!("tool-{start}"))
            .role(Role::Group)
            .aria_label(format!("{state_label} tool call: {caption}"))
            .flex()
            .w_full()
            .min_w_0()
            .items_center()
            .gap_1p5()
            .py_1()
            .line_height(px(20.))
            // React's activity rows are always bare: no status glyph, only
            // a failure stays marked.
            .when(shown.action != "Run", |d| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .text_color(if failed {
                            rgb(0xf87171).into()
                        } else {
                            content(0.5)
                        })
                        .child(action.to_owned()),
                )
            })
            .child(
                div().min_w_0().flex_1().flex().child(
                    div()
                        .id(gpui::SharedString::from(format!("tool-target:{identity}")))
                        .debug_selector(move || format!("tool-target-{start}"))
                        .min_w_0()
                        .when(is_command, |d| d.flex_1())
                        .flex()
                        .items_center()
                        .gap_1()
                        // A chain's description is prose; only its chips are code.
                        .when(!chain, |d| {
                            d.font_family(vorkspace_ui::fonts::FIXED_FONT).text_size(px(12.))
                        })
                        .text_color(if failed {
                            rgb(0xff6467).into()
                        } else {
                            content(0.7)
                        })
                        .when(is_search, |d| {
                            let patterns = shown.search_patterns();
                            let hidden = patterns.len().saturating_sub(2);
                            d.children(patterns.iter().take(2).enumerate().map(
                                |(index, pattern)| {
                                    search_pill()
                                        .debug_selector(move || {
                                            format!("tool-search-pill-{start}-{index}")
                                        })
                                        .child(
                                            div().min_w_0().truncate().child((*pattern).to_owned()),
                                        )
                                },
                            ))
                            .when(hidden > 0, |d| {
                                d.child(
                                    search_pill()
                                        .id(gpui::SharedString::from(format!(
                                            "tool-search-more:{identity}"
                                        )))
                                        .debug_selector(move || format!("tool-search-more-{start}"))
                                        .flex_shrink_0()
                                        .bg(content(0.))
                                        .border_color(content(0.12))
                                        .text_color(if failed {
                                            rgb(0xff6467).into()
                                        } else {
                                            content(0.5)
                                        })
                                        .aria_label(format!(
                                            "{hidden} more patterns: {}",
                                            patterns[2..].join(", ")
                                        ))
                                        .child(format!("+{hidden}")),
                                )
                            })
                        })
                        .when(is_file, |d| {
                            d.rounded(px(4.)).px_1().bg(content(0.06)).child(
                                if shown.action == "List" {
                                    icon("folder", 16.).into_any_element()
                                } else {
                                    crate::activity::file_icon(&shown.target, 16.)
                                },
                            )
                        })
                        .when_some(target_path, |d, path| {
                            d.role(Role::Button)
                                .aria_label(format!("Open {path}"))
                                .cursor_pointer()
                                .hover(|style| style.text_color(rgb(0x7dd3fc)))
                                .on_click(move |_, window, cx| {
                                    cx.stop_propagation();
                                    window.dispatch_action(
                                        Box::new(crate::file_tools::file_editor::OpenPath(path.clone())),
                                        cx,
                                    );
                                })
                        })
                        .when(!is_search && !chain, |d| {
                            d.child(
                                div()
                                    .min_w_0()
                                    .when(is_command, |d| {
                                        // Constrain layout, not content: GPUI can wrap
                                        // even unspaced tokens without changing text.
                                        d.flex_1()
                                            .debug_selector(move || format!("tool-command-{start}"))
                                            .child(gpui::Text::new(
                                                gpui::SharedString::from(format!(
                                                    "tool-command:{identity}"
                                                ))
                                                .into(),
                                                shown.target.clone().into(),
                                            ))
                                    })
                                    .when(!is_command, |d| d.truncate().child(shown.target)),
                            )
                        })
                        .when(chain, |d| {
                            let hidden = chips.len().saturating_sub(3);
                            let rest: Vec<_> = chips
                                .iter()
                                .skip(3)
                                .map(|(_, label, _)| label.clone())
                                .collect();
                            d.gap_1p5()
                                .when_some(shown.description.clone(), |d, description| {
                                    d.child(
                                        div()
                                            .min_w(px(64.))
                                            .flex_shrink(1.)
                                            .truncate()
                                            .child(description),
                                    )
                                })
                                .children(chips.into_iter().take(3).enumerate().map(
                                    |(index, (action, label, path))| {
                                        search_pill()
                                            .id(gpui::SharedString::from(format!(
                                                "tool-step-chip:{identity}:{index}"
                                            )))
                                            .debug_selector(move || {
                                                format!("tool-step-chip-{start}-{index}")
                                            })
                                            .max_w(px(200.))
                                            .flex_shrink(1.)
                                            .font_family(vorkspace_ui::fonts::FIXED_FONT)
                                            .text_size(px(12.))
                                            .gap_1()
                                            .px_1p5()
                                            .when(matches!(action, "Read" | "List"), |d| {
                                                d.child(if action == "List" {
                                                    icon("folder", 14.).into_any_element()
                                                } else {
                                                    crate::activity::file_icon(&label, 14.)
                                                })
                                            })
                                            .when(action == "Find", |d| {
                                                d.child(
                                                    icon("search", 12.)
                                                        .text_color(content(0.45))
                                                        .flex_shrink_0(),
                                                )
                                            })
                                            .child(div().min_w_0().truncate().child(label.clone()))
                                            .when_some(path, |d, path| {
                                                d.role(Role::Button)
                                                    .aria_label(format!("Open {path}"))
                                                    .cursor_pointer()
                                                    .hover(|style| style.text_color(rgb(0x7dd3fc)))
                                                    .on_click(move |_, window, cx| {
                                                        cx.stop_propagation();
                                                        window.dispatch_action(
                                                            Box::new(crate::file_tools::file_editor::OpenPath(
                                                                path.clone(),
                                                            )),
                                                            cx,
                                                        );
                                                    })
                                            })
                                    },
                                ))
                                .when(hidden > 0, |d| {
                                    d.child(
                                        search_pill()
                                            .id(gpui::SharedString::from(format!(
                                                "tool-step-more:{identity}"
                                            )))
                                            .debug_selector(move || {
                                                format!("tool-step-more-{start}")
                                            })
                                            .flex_shrink_0()
                                            .font_family(vorkspace_ui::fonts::FIXED_FONT)
                                            .text_size(px(12.))
                                            .bg(content(0.))
                                            .border_color(content(0.12))
                                            .text_color(if failed {
                                                rgb(0xff6467).into()
                                            } else {
                                                content(0.5)
                                            })
                                            .aria_label(format!(
                                                "{hidden} more steps: {}",
                                                rest.join(", ")
                                            ))
                                            .child(format!("+{hidden}")),
                                    )
                                })
                        }),
                ),
            )
            .when(failed, |d| {
                d.child(icon("close", 14.).text_color(rgb(0xff6467)).flex_shrink_0())
            })
            .when(has_detail, |d| {
                let toggle_key = error_key.clone();
                d.cursor_pointer()
                    .child(
                        div()
                            .id("error-toggle")
                            .role(Role::Button)
                            .aria_expanded(error_open)
                            .aria_label(format!(
                                "{} {} for {caption}",
                                if error_open { "Hide" } else { "Show" },
                                if has_error { "error details" } else { "steps" }
                            ))
                            .p_1()
                            .child(
                                icon(
                                    if error_open {
                                        "chevron-down"
                                    } else {
                                        "chevron-right"
                                    },
                                    14.,
                                )
                                .text_color(if has_error {
                                    rgb(0xf87171).into()
                                } else {
                                    content(0.4)
                                }),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_agent_part(toggle_key.clone(), !error_open, start, cx);
                    }))
            });
        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .child(summary)
            .when(chain && error_open, |d| {
                d.child(
                    div()
                        .debug_selector(move || format!("tool-steps-{start}"))
                        .min_w_0()
                        .pb_2()
                        .flex()
                        .flex_col()
                        .font_family(vorkspace_ui::fonts::FIXED_FONT)
                        .text_size(px(12.))
                        .line_height(px(20.))
                        .children(steps.into_iter().enumerate().map(
                            |(index, (op, command, path))| {
                                div()
                                    .id(gpui::SharedString::from(format!(
                                        "tool-step:{identity}:{index}"
                                    )))
                                    .debug_selector(move || format!("tool-step-{start}-{index}"))
                                    .flex()
                                    .min_w_0()
                                    .gap_2()
                                    .child(
                                        div()
                                            .w(px(16.))
                                            .flex_shrink_0()
                                            .flex()
                                            .justify_end()
                                            .text_color(content(0.3))
                                            .child(op.unwrap_or("")),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .debug_selector(move || {
                                                format!("tool-step-command-{start}-{index}")
                                            })
                                            .text_color(content(0.6))
                                            .child(gpui::Text::new(
                                                gpui::SharedString::from(format!(
                                                    "tool-step-command:{identity}:{index}"
                                                ))
                                                .into(),
                                                command.into(),
                                            )),
                                    )
                                    .when_some(path, |d, path| {
                                        d.role(Role::Button)
                                            .aria_label(format!("Open {path}"))
                                            .cursor_pointer()
                                            .hover(|style| style.text_color(rgb(0x7dd3fc)))
                                            .on_click(move |_, window, cx| {
                                                cx.stop_propagation();
                                                window.dispatch_action(
                                                    Box::new(
                                                        crate::file_tools::file_editor::OpenPath(
                                                            path.clone(),
                                                        ),
                                                    ),
                                                    cx,
                                                );
                                            })
                                    })
                            },
                        )),
                )
            })
            .when(has_error && error_open, |d| {
                d.child(
                    div()
                        .min_w_0()
                        .pb_2()
                        .font_family(vorkspace_ui::fonts::FIXED_FONT)
                        .text_size(px(12.))
                        .line_height(px(20.))
                        .text_color(rgb(0xf87171))
                        .child(error_detail),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
#[path = "tools_wrapping_tests.rs"]
mod wrapping_tests;

#[cfg(test)]
mod tests;
