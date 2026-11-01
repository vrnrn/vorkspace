//! The original PlanPreview actions, backed by host-owned plan identities.
use super::*;
use gpui::AnyElement;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Default, Deserialize, Serialize)]
pub(crate) struct Plan {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub streaming: bool,
    #[serde(default)]
    pub status: String,
}
impl Plan {
    pub(crate) fn can_build(&self, busy: bool) -> bool {
        !busy
            && !self.streaming
            && !self.text.trim().is_empty()
            && !matches!(self.status.as_str(), "streaming" | "building" | "built")
    }
    pub(crate) fn build_label(&self) -> &'static str {
        match self.status.as_str() {
            "building" => "Building…",
            "built" => "Built",
            _ => "Build",
        }
    }
    pub(crate) fn title(&self) -> String {
        self.text
            .lines()
            .find_map(|line| {
                let line = line.trim();
                if line.starts_with('#') {
                    Some(line.trim_start_matches('#').trim().to_owned())
                } else {
                    None
                }
            })
            .or_else(|| {
                self.text
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty() && !line.starts_with("```") && *line != "---")
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "Plan".into())
            .chars()
            .take(80)
            .collect()
    }
    pub(crate) fn summary(&self) -> String {
        let title = self.title();
        let text = self
            .text
            .lines()
            .map(str::trim)
            .filter(|line| {
                !line.is_empty()
                    && !line.starts_with('#')
                    && !line.starts_with("```")
                    && *line != title
            })
            .take(3)
            .collect::<Vec<_>>()
            .join(" ");
        if text.chars().count() > 140 {
            format!("{}…", text.chars().take(139).collect::<String>())
        } else {
            text
        }
    }
    pub(crate) fn command(&self) -> Value {
        json!({"type":"send","intent":"build","planBlockId":self.id})
    }
    pub(crate) fn prompt(&self) -> String {
        format!("Build the approved plan:\n\n{}", self.text)
    }
}

impl TranscriptView {
    pub(super) fn remote_plan_card(&self, plan: &Plan, cx: &mut Context<Self>) -> AnyElement {
        let open = plan.clone();
        let build = plan.clone();
        let enabled = plan.can_build(self.streaming);
        let summary = plan.summary();
        div()
            .id(SharedString::from(format!("remote-plan-{}", plan.id)))
            .w_full()
            .my_2()
            .p_3()
            .rounded(px(12.))
            .border_1()
            .border_color(content(0.1))
            .bg(content(0.06))
            .flex()
            .items_start()
            .gap_2()
            .child(
                icon(
                    if plan.streaming {
                        "loader"
                    } else {
                        "file-text"
                    },
                    16.,
                )
                .text_color(content(0.45)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(content(0.9))
                            .truncate()
                            .child(plan.title()),
                    )
                    .when(!summary.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(px(12.))
                                .line_height(px(18.))
                                .text_color(content(0.5))
                                .child(summary),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                crate::controls::secondary_button(
                                    SharedString::from(format!("open-plan-{}", plan.id)),
                                    "Open plan",
                                    false,
                                    false,
                                )
                                .child("Open")
                                .on_click(cx.listener(
                                    move |_, _, _, cx| {
                                        cx.emit(ResponseRequest::RemotePlan {
                                            id: open.id.clone(),
                                            text: open.text.clone(),
                                            build: false,
                                        })
                                    },
                                )),
                            )
                            .child(
                                crate::controls::secondary_button(
                                    SharedString::from(format!("build-plan-{}", plan.id)),
                                    "Build approved plan",
                                    false,
                                    !enabled,
                                )
                                .child(plan.build_label())
                                .on_click(cx.listener(
                                    move |_, _, _, cx| {
                                        if enabled {
                                            cx.emit(ResponseRequest::RemotePlan {
                                                id: build.id.clone(),
                                                text: build.text.clone(),
                                                build: true,
                                            });
                                        }
                                    },
                                )),
                            ),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[gpui::test]
    fn remote_plans_render_as_cards_in_their_source_position(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let view=cx.new(|cx| {
                let mut view=TranscriptView::new();
                view.patch(0,vec![
                    Row{role:"user".into(),text:"Make a plan".into(),first:true,sent_at:None},
                    Row{role:"remote-plan".into(),text:serde_json::json!({"id":"host-plan","text":"# Plan\n\n- Step","status":"ready","streaming":false}).to_string(),first:true,sent_at:None},
                    Row{role:"assistant".into(),text:"Ready for review".into(),first:true,sent_at:None}
                ],cx);
                view
            });
            let cards:Vec<_>=view.read(cx).rows.iter().filter(|row|matches!(row.body,Body::RemotePlan(_))).collect();
            assert_eq!(cards.len(),1);
            assert_eq!(cards[0].key.0,1);
            assert_eq!(view.read(cx).source_rows()[2].text,"Ready for review");
        });
    }
}
