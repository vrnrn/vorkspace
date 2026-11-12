//! Direct presentation port of AgentTranscript's ActivityPhaseGroup and PhaseStep.
use gpui::{
    AnyElement, App, Bounds, ContentMask, Element, ElementId, GlobalElementId, InspectorElementId,
    LayoutId, Pixels, StyledText, Window, point, prelude::*,
};
use std::collections::HashSet;
use vorkspace_core::tool::{ToolCall, ToolState};

/// The working label: its text shaded by a band of light that sweeps across
/// it, drawn on its own clock while it is on screen.
pub(crate) fn shimmer(text: String, elapsed: f32) -> AnyElement {
    GlyphShimmer {
        key: Some(gpui::SharedString::new_static("shimmer:clock")),
        text: StyledText::new(text),
        elapsed,
    }
    .into_any_element()
}

/// Steps of the sweep per second. GPUI redraws a view's ancestors along with
/// it, so this is the cost of the animation: at 20 the band moves less per
/// step than its soft edge spans, which reads as a glide.
const SHIMMER_FPS: u64 = 20;

/// Shades a fixed glyph layout. Animated StyledText highlights would change
/// font-run boundaries (and kerning/ligatures) as neighbouring colours meet.
struct GlyphShimmer {
    /// Names the clock; `None` draws one still frame.
    key: Option<gpui::SharedString>,
    text: StyledText,
    elapsed: f32,
}
impl IntoElement for GlyphShimmer {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for GlyphShimmer {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        // A clock alive only while the label is drawn: it wakes the view that
        // drew it, which recomputes `elapsed`. Reduce Motion leaves it still.
        if let Some(key) = self.key.clone().filter(|_| !cx.reduce_motion()) {
            window.use_keyed_state(key, cx, |_, cx| {
                cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(1000 / SHIMMER_FPS))
                            .await;
                        if this.update(cx, |_, cx| cx.notify()).is_err() {
                            break;
                        }
                    }
                })
            });
        }
        self.text.request_layout(None, None, window, cx)
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.text.prepaint(None, None, bounds, state, window, cx);
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) {
        let layout = self.text.layout();
        let line_height = layout.line_height();
        // Callers use a single truncated line; StyledText's layout keeps
        // overflow, font fallback and plain-text geometry identical.
        let lines = layout.line_layouts();
        let elapsed = self.elapsed;
        window.paint_layer(bounds, |window| {
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                for wrapped in &lines {
                    let line = &wrapped.unwrapped_layout;
                    let baseline = (line_height - line.ascent - line.descent) / 2. + line.ascent;
                    let width = f32::from(line.width).max(1.);
                    for run in &line.runs {
                        for glyph in &run.glyphs {
                            // Sample at the glyph's middle so a letter lights
                            // as the band's centre crosses it.
                            let x = f32::from(glyph.position.x + line.font_size * 0.25) / width;
                            let lit = vorkspace_ui::fluid::shimmer_sweep(x, elapsed);
                            let origin = bounds.origin
                                + point(glyph.position.x, baseline + glyph.position.y);
                            if glyph.is_emoji {
                                let _ = window.paint_emoji(
                                    origin,
                                    run.font_id,
                                    glyph.id,
                                    line.font_size,
                                );
                            } else {
                                let _ = window.paint_glyph(
                                    origin,
                                    run.font_id,
                                    glyph.id,
                                    line.font_size,
                                    vorkspace_ui::theme::content(0.4 + 0.6 * lit),
                                );
                            }
                        }
                    }
                }
            });
        });
    }
}

pub(crate) fn file_icon(path: &str, side: f32) -> AnyElement {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    vorkspace_ui::file_icons::file_type_icon(name, false, false, side)
}
pub(crate) fn summary(tools: &[ToolCall], live: bool) -> (String, &'static str) {
    let mut order = Vec::new();
    let mut reads = HashSet::new();
    let mut edits = HashSet::new();
    let mut searches = 0;
    let mut read_calls = 0;
    let mut edit_calls = 0;
    let mut commands = 0;
    let mut agents = 0;
    let mut failed_agents = 0;
    let mut others = 0;
    let mut current = "think";
    for tool in tools {
        let (action, target) = tool.summary();
        let kind = match action.as_str() {
            "Read" | "List" | "Search" | "Find" => "research",
            "Edit" | "Write" => "edit",
            "Run" => "run",
            "Agent" => "agent",
            _ => "other",
        };
        if !order.contains(&kind) {
            order.push(kind);
        }
        if live {
            current = kind;
        }
        match action.as_str() {
            "Read" => {
                read_calls += 1;
                reads.insert(target);
            }
            "Edit" | "Write" => {
                edit_calls += 1;
                edits.insert(target);
            }
            "Search" | "Find" | "List" => searches += 1,
            "Run" => commands += 1,
            "Agent" => {
                agents += 1;
                if matches!(tool.state, ToolState::Failed | ToolState::Cancelled) {
                    failed_agents += 1;
                }
            }
            _ => others += 1,
        }
    }
    if failed_agents > 0 {
        return (
            if failed_agents == 1 {
                "Subagent failed".into()
            } else {
                format!("{failed_agents} subagents failed")
            },
            "bot",
        );
    }
    let file_label = |paths: &HashSet<String>| {
        if paths.len() == 1 {
            paths
                .iter()
                .next()
                .unwrap()
                .rsplit('/')
                .next()
                .unwrap_or("")
                .to_owned()
        } else {
            format!("{} files", paths.len())
        }
    };
    let title = order
        .iter()
        .map(|kind| {
            let live = *kind == current;
            match *kind {
                "research" if searches > 0 && !reads.is_empty() => if live {
                    "Exploring the project"
                } else {
                    "Explored the project"
                }
                .into(),
                "research" if !reads.is_empty() => format!(
                    "{} {}",
                    if live { "Reading" } else { "Read" },
                    file_label(&reads)
                ),
                "research" => if live {
                    "Searching the project"
                } else {
                    "Searched the project"
                }
                .into(),
                "edit" => format!(
                    "{} {}",
                    if live { "Editing" } else { "Edited" },
                    file_label(&edits)
                ),
                "run" => format!(
                    "{} {}",
                    if live { "Running" } else { "Ran" },
                    if commands == 1 {
                        "a command".into()
                    } else {
                        format!("{commands} commands")
                    }
                ),
                "agent" => format!(
                    "{} {}",
                    if live { "Running" } else { "Ran" },
                    if agents == 1 {
                        "a subagent".into()
                    } else {
                        format!("{agents} subagents")
                    }
                ),
                _ => format!(
                    "{} {}",
                    if live { "Running" } else { "Ran" },
                    if others == 1 {
                        "a tool".into()
                    } else {
                        format!("{others} tools")
                    }
                ),
            }
        })
        .collect::<Vec<String>>()
        .join(" · ");
    // React breaks equal call counts towards edit, run, agent, then research.
    let mut dominant = "terminal";
    let mut most = 0;
    for (count, icon) in [
        (edit_calls, "pencil-line"),
        (commands, "terminal"),
        (agents, "bot"),
        (read_calls + searches, "search"),
        (others, "terminal"),
    ] {
        if count > most {
            most = count;
            dominant = icon;
        }
    }
    (title, dominant)
}
/// CSS .zen-phase-step: left 6px spine, 8px quarter curve, 20px content inset.
pub(crate) fn rail(last: bool) -> impl IntoElement {
    rail_branch(last, true)
}
pub(crate) fn rail_branch(last: bool, branch: bool) -> impl IntoElement {
    rail_branch_at(last, branch, 0.)
}
/// The same rail for a step whose content sits `offset` lower, as the
/// sidebar's `.orchestration-tree-step` does with its 2px padding.
pub(crate) fn rail_branch_at(last: bool, branch: bool, offset: f32) -> impl IntoElement {
    // Theme ink, so the rail is as quiet on paper as it is on dark.
    let color = vorkspace_ui::theme::content(0.14);
    gpui::div()
        .absolute()
        .top_0()
        .left(gpui::px(6.))
        .w(gpui::px(9.))
        .h_full()
        .child(
            gpui::div()
                .absolute()
                .top_0()
                .left_0()
                .w(gpui::px(1.))
                .map(|d| {
                    if last {
                        d.h(gpui::px(7. + offset))
                    } else {
                        d.h_full()
                    }
                })
                .bg(color),
        )
        .when(branch, |d| {
            d.child(
                gpui::svg()
                    .path("tool-branch")
                    .absolute()
                    .left_0()
                    .top(gpui::px(6. + offset))
                    .size(gpui::px(9.))
                    .text_color(color),
            )
        })
}

#[cfg(test)]
mod shimmer_tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext, canvas, div, px};
    use std::{cell::RefCell, rc::Rc};

    struct Preview {
        elapsed: f32,
        geometry: Rc<RefCell<Option<(Pixels, String)>>>,
    }
    impl Render for Preview {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let elapsed = self.elapsed;
            let geometry = self.geometry.clone();
            div()
                .text_size(px(14.))
                .line_height(px(20.))
                .truncate()
                .child(
                    canvas(
                        move |_, window, cx| {
                            let text = "GPT-6.1-Sol working for 29s · office AV café 漢字";
                            let shimmer = GlyphShimmer {
                                key: None,
                                text: StyledText::new(text),
                                elapsed,
                            };
                            let layout = shimmer.text.layout().clone();
                            let mut shimmer = shimmer.into_any_element();
                            let available = gpui::size(
                                gpui::AvailableSpace::Definite(px(220.)),
                                gpui::AvailableSpace::MinContent,
                            );
                            let measured = shimmer.layout_as_root(available, window, cx);
                            let plain = StyledText::new(text);
                            let plain_layout = plain.layout().clone();
                            let plain = plain
                                .into_any_element()
                                .layout_as_root(available, window, cx);
                            assert_eq!(measured, plain);
                            let glyphs = |layout: &gpui::TextLayout| {
                                layout
                                    .line_layouts()
                                    .iter()
                                    .map(|line| format!("{:?}", line.unwrapped_layout.runs))
                                    .collect::<String>()
                            };
                            assert_eq!(glyphs(&layout), glyphs(&plain_layout));
                            *geometry.borrow_mut() = Some((measured.width, glyphs(&layout)));
                        },
                        |_, _, _, _| {},
                    )
                    .w_full()
                    .h(px(20.)),
                )
        }
    }
    #[gpui::test]
    fn sweep_preserves_plain_text_kerning_ligatures_and_glyph_positions(cx: &mut TestAppContext) {
        let geometry = Rc::new(RefCell::new(None));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| Preview {
                    elapsed: 0.,
                    geometry: geometry.clone(),
                })
            })
            .unwrap()
        });
        let mut first = None;
        for elapsed in [0., 0.2, 0.6, 1., 1.59, 1.6] {
            window
                .update(cx, |view, _, cx| {
                    view.elapsed = elapsed;
                    cx.notify();
                })
                .unwrap();
            cx.refresh().unwrap();
            cx.run_until_parked();
            let current = geometry.borrow().clone().expect("shimmer was measured");
            if let Some(first) = &first {
                assert_eq!(first, &current);
            } else {
                first = Some(current);
            }
        }
    }
}

#[cfg(test)]
mod shimmer_clock_tests {
    use gpui::{TestAppContext, VisualTestContext, prelude::*};

    struct Label {
        renders: usize,
    }
    impl gpui::Render for Label {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            self.renders += 1;
            gpui::div().child(super::shimmer("Thinking".into(), 0.))
        }
    }

    #[gpui::test]
    fn the_shimmer_steps_on_its_own_clock_and_stops_with_reduce_motion(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| cx.new(|_| Label { renders: 0 }))
                .unwrap()
        });
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let renders = |visual: &mut VisualTestContext| {
            window.update(visual, |label, _, _| label.renders).unwrap()
        };
        let step = |visual: &mut VisualTestContext| {
            let before = renders(visual);
            visual
                .executor()
                .advance_clock(std::time::Duration::from_millis(
                    1000 / super::SHIMMER_FPS * 4,
                ));
            visual.run_until_parked();
            renders(visual) - before
        };
        let moved = step(visual);
        assert!(moved > 0, "the label redraws as the band moves");
        assert!(moved <= 4, "no more often than the shimmer's frame rate");
        visual.update(|_, cx| cx.set_reduce_motion(true));
        window.update(visual, |_, _, cx| cx.notify()).unwrap();
        visual.run_until_parked();
        let _ = step(visual);
        assert_eq!(
            step(visual),
            0,
            "under Reduce Motion nothing wakes the view"
        );
    }
}

#[cfg(test)]
mod shimmer_frames {
    use super::*;
    use gpui::{Context, Render, div, px};
    use std::sync::Arc;

    struct Frames;
    impl Render for Frames {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .p(px(16.))
                .bg(gpui::rgb(0x161616))
                .text_color(vorkspace_ui::theme::content(1.))
                .text_size(px(14.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .children((0..16).map(|i| {
                    let t = i as f32 * 0.1;
                    div()
                        .id(i)
                        .flex()
                        .gap(px(12.))
                        .child(div().w(px(40.)).child(format!("{t:.1}")))
                        .child(
                            div()
                                .w(px(320.))
                                .child(shimmer("Thinking about the layout".into(), t)),
                        )
                }))
        }
    }

    /// Writes one image of the shimmer at sixteen moments of its sweep.
    #[test]
    #[ignore = "Requires Metal and VORKSPACE_SHIMMER_FRAMES"]
    fn shimmer_frames_capture() {
        let out = std::path::PathBuf::from(std::env::var_os("VORKSPACE_SHIMMER_FRAMES").unwrap());
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
        vorkspace_ui::theme::configure(&Default::default(), true);
        let window = cx
            .open_window(gpui::size(px(420.), px(440.)), |_, cx| cx.new(|_| Frames))
            .unwrap();
        cx.run_until_parked();
        cx.capture_screenshot(window.into())
            .unwrap()
            .save(&out)
            .unwrap();
    }
}
