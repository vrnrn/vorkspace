use super::*;
use gpui::{AnyElement, Div, ElementId, EventEmitter, Stateful};
use serde_json::{Value, json};
pub(crate) struct CardAction(pub Value);
impl EventEmitter<CardAction> for crate::transcript::TranscriptView {}
/// The chat's one-line cards: a 12px-round outline that brightens on hover.
fn line_card(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Button)
        .flex()
        .max_w(px(384.))
        .min_w_0()
        .items_center()
        .gap_2p5()
        .rounded_xl()
        .border_1()
        .border_color(content(0.1))
        .line_height(px(20.))
        .hover(|s| s.border_color(content(0.2)).bg(content(0.05)))
}
/// ArtifactCard: the document's icon and title, opening it in the reader.
fn artifact(value: &Value, cx: &mut Context<crate::transcript::TranscriptView>) -> AnyElement {
    let html = value["kind"] == "html";
    let label = if html { "visual" } else { "document" };
    let title = value["title"].as_str().unwrap_or("Document").to_string();
    let card = value.clone();
    div()
        .flex()
        .child(
            line_card(SharedString::from(format!(
                "bot-artifact-{}",
                value["id"].as_str().unwrap_or_default()
            )))
            .aria_label(format!("Open {label} {title}"))
            .debug_selector(|| "bot-artifact-card".into())
            .min_h(px(38.))
            .py_2()
            .pl_3()
            .pr(px(14.))
            .child(icon(if html { "chart-square" } else { "file" }, 16.).text_color(content(0.55)))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .text_color(content(1.))
                    .child(title),
            )
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(CardAction(card.clone())))),
        )
        .into_any_element()
}
/// LaunchedSessionCard: its status, title and the provider and project it
/// runs in. Without a known state it shows the plain chat mark.
pub(crate) fn launch(
    id: i64,
    view: Option<&crate::transcript::LaunchView>,
    spin: f32,
    cx: &mut Context<crate::transcript::TranscriptView>,
) -> AnyElement {
    let title = view.map_or("Agent session".to_string(), |v| v.title.clone());
    let (label, glyph, color) = match view.map(|v| v.state) {
        Some("working") => ("Working", "loader-03", content(0.55)),
        Some("input") => ("Needs input", "clock", rgb(0xfbbf24).into()),
        Some("draft") => ("Draft", "circle-dashed-hi", content(0.45)),
        Some("failed") => ("Failed", "alert-circle", rgb(0xf87171).into()),
        Some("stopped") => ("Stopped", "circle-x", content(0.45)),
        Some("unavailable") => ("Unavailable", "chatting", content(0.45)),
        Some(_) => ("Done", "check-circle", {
            let mut c: gpui::Hsla = rgb(0x34d399).into();
            c.a = 0.9;
            c
        }),
        None => ("", "chatting", content(0.45)),
    };
    let mark = icon(glyph, 16.).text_color(color);
    let mark = if glyph == "loader-03" {
        mark.with_transformation(gpui::Transformation::rotate(gpui::percentage(spin.fract())))
    } else {
        mark
    };
    let project = view
        .map(|v| {
            std::path::Path::new(&v.cwd)
                .file_name()
                .map_or(v.cwd.clone(), |n| n.to_string_lossy().to_string())
        })
        .unwrap_or_default();
    let harness = view.map(|v| v.harness.clone()).unwrap_or_default();
    div()
        .flex()
        .child(
            line_card(("bot-launched-session", id as usize))
                .aria_label(if label.is_empty() {
                    format!("Open session {title}")
                } else {
                    format!("Open session {title}, {}", label.to_lowercase())
                })
                .debug_selector(move || format!("bot-launch-card-{id}"))
                .py(px(6.))
                .pl_3()
                .pr(px(6.))
                .child(mark)
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(13.))
                        .text_color(content(1.))
                        .child(title),
                )
                .when(view.is_some(), |d| {
                    d.child(
                        div()
                            .flex()
                            .flex_shrink_0()
                            .max_w(px(160.))
                            .min_w_0()
                            .items_center()
                            .gap_1()
                            .rounded_full()
                            .border_1()
                            .border_color(content(0.1))
                            .bg(content(0.05))
                            .px_2()
                            .py(px(2.))
                            .text_size(px(12.))
                            .text_color(content(0.65))
                            .child(provider_icon(&harness, 14.))
                            .child(div().min_w_0().truncate().child(project)),
                    )
                })
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(CardAction(json!({"type":"session","sessionId":id})))
                })),
        )
        .into_any_element()
}
pub(crate) fn render(
    value: &Value,
    cx: &mut Context<crate::transcript::TranscriptView>,
) -> AnyElement {
    let kind = value["type"].as_str().unwrap_or_default();
    match kind {
        "document" => return artifact(value, cx),
        "session" => {
            let id = value["sessionId"].as_i64().unwrap_or_default();
            let view = value["state"]
                .as_str()
                .map(|state| crate::transcript::LaunchView {
                    title: value["title"].as_str().unwrap_or("Agent session").into(),
                    state: match state {
                        "working" => "working",
                        "input" => "input",
                        "draft" => "draft",
                        "failed" => "failed",
                        "stopped" => "stopped",
                        _ => "done",
                    },
                    harness: value["harness"].as_str().unwrap_or_default().into(),
                    cwd: value["cwd"].as_str().unwrap_or_default().into(),
                });
            return launch(id, view.as_ref(), 0., cx);
        }
        _ => {}
    }
    let title = value["title"]
        .as_str()
        .or_else(|| value["name"].as_str())
        .unwrap_or(match kind {
            "pr" => "Pull request",
            "session" => "Session",
            "habit" => "Suggested habit",
            _ => "",
        })
        .to_string();
    let card = value.clone();
    let glyph = match kind {
        "pr" => "git-pull-request",
        "session" => "message-square",
        "habit" => "clock",
        _ => "file",
    };
    div()
        .rounded_lg()
        .border_1()
        .border_color(content(0.12))
        .bg(content(0.035))
        .p_3()
        .max_w(px(480.))
        .flex()
        .flex_col()
        .gap_2()
        .when(kind != "choices", |d| {
            d.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(13.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(icon(glyph, 16.))
                    .child(title),
            )
        })
        .when_some(
            value["note"].as_str().or_else(|| value["summary"].as_str()),
            |d, n| {
                d.child(
                    div()
                        .text_size(px(12.))
                        .text_color(content(0.55))
                        .child(n.to_string()),
                )
            },
        )
        .when(kind == "session", |d| {
            d.child(
                div()
                    .text_size(px(12.))
                    .text_color(content(0.5))
                    .child(value["state"].as_str().unwrap_or("Saved").to_string()),
            )
        })
        .when(kind == "pr", |d| {
            d.child(
                div()
                    .text_size(px(12.))
                    .text_color(content(0.5))
                    .child(format!(
                        "{} #{} · {}",
                        value["repo"].as_str().unwrap_or_default(),
                        value["number"],
                        value["state"].as_str().unwrap_or("GitHub")
                    )),
            )
        })
        .when(kind == "habit", |d| {
            d.child(
                div().text_size(px(12.)).text_color(content(0.5)).child(
                    serde_json::from_value::<vorkspace_core::bot::habits::Schedule>(
                        value["schedule"].clone(),
                    )
                    .map(|s| s.label())
                    .unwrap_or_default(),
                ),
            )
        })
        .when(kind == "choices", |d| {
            d.children(
                value["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .filter_map(|(ix, v)| {
                        v.as_str().map(|label| {
                            let label = label.to_string();
                            let answer = label.clone();
                            crate::controls::bot_button(("bot-choice", ix), label)
                                .bg(content(0.06))
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(CardAction(json!({"type":"choice","text":answer})))
                                }))
                        })
                    }),
            )
        })
        .when(kind != "choices", |d| {
            d.child(
                crate::controls::bot_button(
                    "bot-card-open",
                    if kind == "habit" && value["started"] != true {
                        "Start habit"
                    } else {
                        "Open"
                    },
                )
                .bg(content(0.08))
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(CardAction(card.clone())))),
            )
        })
        .into_any_element()
}
