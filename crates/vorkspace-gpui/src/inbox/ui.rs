use super::*;
use crate::popover::{Align, Anchor, popover};
use gpui::{
    AnyElement, ClipboardItem, CursorStyle, Div, FontWeight, Hsla, MouseButton, Pixels, Role,
    SharedString, Stateful, Window, canvas, div, list, prelude::*, px, rgb,
};

fn action(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Stateful<Div> {
    let id: SharedString = id.into();
    let label = label.into();
    crate::controls::button(id.clone(), label.clone(), true)
        .debug_selector(move || id.to_string())
        .h(px(28.))
        .px(px(10.))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .rounded(px(6.))
        .text_size(px(12.))
        .text_color(content(0.7))
        .cursor_pointer()
        .hover(|s| s.bg(content(0.07)).text_color(content(1.)))
        .child(label)
}
fn icon_action(id: impl Into<SharedString>, label: &str, name: &'static str) -> Stateful<Div> {
    icon_action_ink(id, label, name, content(0.7))
}
fn icon_action_ink(
    id: impl Into<SharedString>,
    label: &str,
    name: &'static str,
    ink: Hsla,
) -> Stateful<Div> {
    let id: SharedString = id.into();
    crate::controls::button(id.clone(), label.to_owned(), true)
        .debug_selector(move || id.to_string())
        .h(px(28.))
        .px(px(10.))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .rounded(px(6.))
        .text_size(px(12.))
        .text_color(content(0.7))
        .cursor_pointer()
        .hover(move |s| {
            if ink == shell_fill() {
                s.bg(shell_fill().opacity(0.1)).text_color(ink)
            } else {
                s.bg(content(0.07)).text_color(content(1.))
            }
        })
        .child(action_icon(name, ink))
        .child(label.to_owned())
}
fn action_icon(name: &'static str, ink: Hsla) -> gpui::Svg {
    icon(name, 14.).text_color(ink)
}
fn relative_time(iso: &str) -> String {
    if model::timestamp_ms(iso) == 0 {
        return String::new();
    }
    let mut value =
        (((model::timestamp_ms(iso) - chrono::Utc::now().timestamp_millis()) as f64 / 1000.) + 0.5)
            .floor();
    let mut unit = "second";
    for (step, name) in [
        (60., "second"),
        (60., "minute"),
        (24., "hour"),
        (7., "day"),
        (4.34524, "week"),
        (12., "month"),
        (f64::INFINITY, "year"),
    ] {
        unit = name;
        if value.abs() < step {
            break;
        }
        value = ((value / step) + 0.5).floor();
    }
    let amount = value as i64;
    match (unit, amount) {
        ("second", 0) => "now".into(),
        ("day", -1) => "yesterday".into(),
        ("day", 1) => "tomorrow".into(),
        ("week" | "month" | "year", -1) => format!("last {unit}"),
        ("week" | "month" | "year", 1) => format!("next {unit}"),
        _ => {
            let count = amount.unsigned_abs();
            let label = format!("{count} {unit}{}", if count == 1 { "" } else { "s" });
            if amount > 0 {
                format!("in {label}")
            } else {
                format!("{label} ago")
            }
        }
    }
}
fn review_label(state: &str) -> String {
    match state.trim().to_uppercase().as_str() {
        "APPROVED" => "Approved".into(),
        "CHANGES_REQUESTED" => "Changes requested".into(),
        "COMMENTED" => "Commented".into(),
        "DISMISSED" => "Dismissed".into(),
        "PENDING" => "Pending".into(),
        "REVIEW_REQUIRED" => "Review required".into(),
        _ => state.replace('_', " ").to_lowercase(),
    }
}
fn failed_check(state: &str) -> bool {
    matches!(
        state.to_lowercase().as_str(),
        "failure" | "failed" | "error" | "timed_out" | "cancelled" | "action_required"
    )
}
fn review_event(comment: &model::Comment) -> bool {
    comment.kind == "review" && comment.body.trim().is_empty() && comment.replies.is_empty()
}
#[derive(Clone)]
pub(super) enum Activity {
    Comment(std::sync::Arc<model::Comment>),
    Commits(Vec<model::Commit>),
}
#[derive(Default)]
pub(super) struct SummaryRows {
    revision: u64,
    discussion: bool,
    keys: Vec<SharedString>,
}
fn timeline_event(name: &str, action: &str, time: &str) -> Div {
    div()
        .h(px(20.))
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(px(12.))
        .text_color(content(0.5))
        .child(
            div()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::MEDIUM)
                .text_color(content(1.))
                .child(if name.is_empty() {
                    "ghost".to_owned()
                } else {
                    name.to_owned()
                }),
        )
        .child(action.to_owned())
        .when(!time.is_empty(), |d| {
            d.child("·").child(relative_time(time))
        })
}
fn menu_row(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    checked: bool,
) -> Stateful<Div> {
    let id: SharedString = id.into();
    let label = label.into();
    div()
        .id(id.clone())
        .debug_selector(move || id.to_string())
        .role(Role::MenuItemCheckBox)
        .aria_label(label.clone())
        .h(px(28.))
        .px(px(8.))
        .flex()
        .items_center()
        .gap(px(8.))
        .rounded(px(6.))
        .text_size(px(12.))
        .text_color(content(0.8))
        .cursor_pointer()
        .hover(|s| s.bg(content(0.06)))
        .child(
            div()
                .size(px(14.))
                .when(checked, |d| d.child(icon("check", 14.))),
        )
        .child(div().flex_1().min_w_0().truncate().child(label))
}
fn caption(text: impl Into<SharedString>) -> Div {
    div()
        .px(px(8.))
        .pt(px(10.))
        .pb(px(4.))
        .text_size(px(10.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(content(0.4))
        .child(text.into())
}
fn message(text: impl Into<SharedString>) -> Div {
    div()
        .p(px(12.))
        .text_size(px(12.))
        .line_height(px(19.))
        .text_color(content(0.5))
        .child(text.into())
}
fn error(text: impl Into<SharedString>) -> Div {
    message(text).text_color(rgb(0xf87171))
}
fn provider_mark(provider: &str) -> gpui::Svg {
    let (path, color) = match provider {
        "jira" => ("inbox-jira", rgb(0x2684ff).into()),
        "gitlab" => ("inbox-gitlab", rgb(0xfc6d26).into()),
        _ => ("inbox-github", content(0.8)),
    };
    icon(path, 14.).text_color(color)
}
fn status_color(item: &Item) -> Hsla {
    match item.status() {
        "Merged" => rgb(0xa78bfa).into(),
        "Closed" => rgb(0xf87171).into(),
        "Draft" => content(0.4),
        _ => rgb(0x4ade80).into(),
    }
}
fn status_icon(item: &Item) -> &'static str {
    if item.kind == "pr" {
        "git-pull-request"
    } else if item.status() == "Closed" {
        "check-circle"
    } else {
        "circle-dot"
    }
}
fn attention_label(reason: &str) -> String {
    let reason = reason.trim().to_lowercase();
    match reason.as_str() {
        "assigned" => "Assigned to you",
        "mentioned" | "directly_addressed" => "Mentioned you",
        "review_requested" => "Review requested",
        "review_submitted" => "Review submitted",
        "approval_required" => "Approval required",
        "build_failed" => "Pipeline failed",
        "unmergeable" => "Cannot be merged",
        "merge_train_removed" => "Removed from merge train",
        "member_access_requested" => "Access requested",
        "marked" => "Added to your to-dos",
        _ => {
            return reason
                .split('_')
                .filter(|word| !word.is_empty())
                .enumerate()
                .map(|(ix, word)| {
                    if ix == 0 {
                        let mut chars = word.chars();
                        chars
                            .next()
                            .map(|c| c.to_uppercase().to_string() + chars.as_str())
                            .unwrap_or_default()
                    } else {
                        word.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
        }
    }
    .into()
}
fn toggle(list: &mut Vec<String>, key: String) {
    if list.contains(&key) {
        list.retain(|value| value != &key);
    } else {
        list.push(key);
    }
}
fn project_name(path: &std::path::Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

fn label_badge(label: &model::Label, compact: bool) -> Div {
    let color = u32::from_str_radix(label.color.trim().trim_start_matches('#'), 16)
        .ok()
        .filter(|_| label.color.trim().trim_start_matches('#').len() == 6);
    div()
        .max_w(px(if compact { 80. } else { 200. }))
        .rounded(px(3.))
        .px(px(6.))
        .py(px(1.))
        .flex()
        .items_center()
        .gap(px(4.))
        .bg(content(0.08))
        .text_size(px(if compact { 10. } else { 11. }))
        .text_color(content(0.5))
        .when_some(color, |d, color| {
            d.child(div().size(px(6.)).rounded_full().bg(rgb(color)))
        })
        .child(div().min_w_0().truncate().child(label.name.clone()))
}

#[derive(Clone, Copy)]
enum CommentStyle {
    Plain,
    Timeline,
    Reply,
}

impl Inbox {
    fn project_mark(&self, path: &str, side: f32) -> AnyElement {
        crate::project_settings::project_icon(
            &self.appearances.get(path).cloned().unwrap_or_default(),
            &project_name(std::path::Path::new(path)),
            side,
        )
    }

    fn list_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let provider = model::provider_label(&self.state.source);
        let connected = self
            .snapshot
            .connections
            .get(&self.state.source)
            .is_some_and(|status| status.connected);
        let items = self.visible_items(cx);
        let tools = div()
            .h(px(36.))
            .px(px(8.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(1.))
            .border_b_1()
            .border_color(content(0.07))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(28.))
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(icon("search", 12.))
                    .child(self.search.clone()),
            )
            .when(connected, |d| {
                d.child(
                    icon_button("inbox-filter", "Filter items", "filter")
                        .relative()
                        .child(self.filter_anchor.probe())
                        .when(self.filters_open || self.filters_active(), |d| {
                            d.bg(content(0.1))
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.filters_open = !this.filters_open;
                            this.connections_open = false;
                            cx.notify();
                        })),
                )
            })
            .when(connected, |d| {
                d.child(
                    icon_button("inbox-read-all", "Mark all as read", "check-2").on_click(
                        cx.listener(|this, _, _, cx| {
                            for item in &this.snapshot.items {
                                if item.provider == this.state.source {
                                    this.state.seen.mark(item);
                                }
                            }
                            this.save(cx);
                            cx.notify();
                        }),
                    ),
                )
            })
            .child(
                icon_button(
                    "inbox-refresh",
                    "Refresh",
                    if self.loading { "loader" } else { "refresh-cw" },
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.refresh(cx);
                    this.load_detail(false, cx);
                })),
            );
        let empty = if !self.state.connectors.enabled(&self.state.source) {
            format!("{provider} is disabled. Enable it in Settings → Connectors.")
        } else if let Some(error) = self.snapshot.errors.get(&self.state.source) {
            format!("{provider}: {error}. Check Settings → Connectors or refresh.")
        } else if self.loading && items.is_empty() {
            format!("Loading {provider}…")
        } else if !connected {
            format!("Connect {provider} in Settings → Connectors.")
        } else if self.projects.is_empty() && !model::is_tracker(&self.state.source) {
            format!("Open a project to see {provider} items.")
        } else if self.filters_active() || !self.search.read(cx).content.is_empty() {
            format!("No {provider} items match these filters.")
        } else {
            format!(
                "No {provider} issues{}.",
                if model::is_tracker(&self.state.source) {
                    ""
                } else if self.state.source == "gitlab" {
                    " or merge requests"
                } else {
                    " or pull requests"
                }
            )
        };
        let rows = items
            .into_iter()
            .map(|item| {
                let key = item.key();
                let active = self.selected.as_ref() == Some(&key);
                let unread = self.state.seen.unseen(&item);
                let sessions = self.related_sessions(&item).len();
                let tracker = model::is_tracker(&item.provider);
                let kind = if item.kind == "pr" {
                    if item.provider == "gitlab" {
                        "Merge request"
                    } else {
                        "Pull request"
                    }
                } else {
                    "Issue"
                };
                let source = if tracker && !item.team_name.is_empty() {
                    item.team_name.clone()
                } else if !item.repo.is_empty() {
                    item.repo.clone()
                } else {
                    project_name(std::path::Path::new(&item.project_path))
                };
                let attention = attention_label(&item.attention_reason);
                div()
                    .id(SharedString::from(format!("inbox-item-{key}")))
                    .role(Role::Button)
                    .aria_label(item.title.clone())
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(content(0.))
                    .cursor_pointer()
                    .when(active, |d| d.bg(content(0.1)))
                    .when(!active, |d| d.hover(|s| s.bg(content(0.05))))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(11.))
                            .text_color(content(0.5))
                            .child(provider_mark(&item.provider))
                            .child(icon(status_icon(&item), 12.).text_color(status_color(&item)))
                            .child(div().min_w_0().flex_1().truncate().child(format!(
                                "{kind} · {}{}",
                                item.reference(),
                                if attention.is_empty() {
                                    String::new()
                                } else {
                                    format!(" · {attention}")
                                }
                            )))
                            .when(sessions > 0, |d| {
                                d.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(2.))
                                        .text_color(accent_color())
                                        .child(
                                            icon("message-multiple", 12.)
                                                .text_color(accent_color()),
                                        )
                                        .child(sessions.to_string()),
                                )
                            })
                            .child(
                                div()
                                    .text_color(content(0.45))
                                    .child(relative_time(&item.updated_at)),
                            )
                            .when(unread, |d| {
                                d.child(div().size(px(6.)).rounded_full().bg(accent_color()))
                            }),
                    )
                    .child(
                        div()
                            .mt(px(4.))
                            .truncate()
                            .text_size(px(13.))
                            .line_height(px(18.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(content(1.))
                            .child(item.title.clone()),
                    )
                    .child(
                        div()
                            .mt(px(4.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .text_size(px(11.))
                                    .text_color(content(0.45))
                                    .when(!tracker && !item.project_path.is_empty(), |d| {
                                        d.child(self.project_mark(&item.project_path, 12.))
                                    })
                                    .child(div().min_w_0().truncate().child(source)),
                            )
                            .children(
                                item.labels
                                    .iter()
                                    .take(2)
                                    .map(|label| label_badge(label, true)),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.select(key.clone(), cx)))
            })
            .collect::<Vec<_>>();
        div()
            .relative()
            .w(px(self.state.list_width.clamp(240., 420.)))
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(content(0.07))
            .flex()
            .flex_col()
            .child(tools)
            .child(
                div()
                    .id("inbox-list-scroll")
                    .debug_selector(|| "connector-list".into())
                    .track_scroll(&self.list_scroll)
                    .role(Role::ListBox)
                    .aria_label(format!("{} items", provider))
                    .min_h_0()
                    .flex_1()
                    .overflow_y_scroll()
                    .p(px(6.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .when(rows.is_empty(), |d| d.child(message(empty)))
                    .children(rows),
            )
            .child(
                div()
                    .id("inbox-list-resize")
                    .role(Role::Splitter)
                    .aria_label("Resize connector list")
                    .absolute()
                    .right(px(-3.))
                    .top_0()
                    .bottom_0()
                    .w(px(6.))
                    .cursor(CursorStyle::ResizeLeftRight)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                            this.resize_start =
                                Some((f32::from(event.position.x), this.state.list_width));
                            cx.stop_propagation();
                        }),
                    ),
            )
            .into_any_element()
    }
    fn related_sessions(&self, item: &Item) -> Vec<Session> {
        let work_item = item.work_item();
        self.sessions
            .iter()
            .filter(|s| {
                s.work_item.as_ref().is_some_and(|work| {
                    (!work.url.is_empty() && work.url == work_item.url)
                        || (work.kind == work_item.kind
                            && work.number == work_item.number
                            && work.repo.eq_ignore_ascii_case(&work_item.repo))
                })
            })
            .cloned()
            .collect()
    }
}

impl gpui::Render for Inbox {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_comment {
            self.focus_comment = false;
            window.focus(&self.comment.focus_handle(cx), cx);
        }
        let list = self.list_view(cx);
        let detail = self.detail_view(window, cx);
        div()
            .id("inbox-view")
            .role(Role::Group)
            .aria_label(model::provider_label(&self.state.source))
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    if this.connect_provider.is_some() && !this.connecting {
                        this.connect_provider = None;
                    } else if this.confirm_action.is_some() && !this.acting {
                        this.confirm_action = None;
                    } else if this.filters_open
                        || this.connections_open
                        || this.project_picker
                        || this.action_menu
                    {
                        this.filters_open = false;
                        this.connections_open = false;
                        this.project_picker = false;
                        this.action_menu = false;
                    } else {
                        cx.emit(Event::Close);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_mouse_move(
                cx.listener(|this, event: &gpui::MouseMoveEvent, window, cx| {
                    if let Some((start, width)) = this.resize_start {
                        let max = (f32::from(window.viewport_size().width) * 0.5).clamp(240., 420.);
                        this.state.list_width =
                            (width + f32::from(event.position.x) - start).clamp(240., max);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.resize_start.take().is_some() {
                        this.save(cx);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.resize_start.take().is_some() {
                        this.save(cx);
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .flex()
                    .child(list)
                    .child(div().flex_1().min_w_0().min_h_0().child(detail)),
            )
            .when_some(self.error.clone(), |d, text| d.child(error(text)))
            .when(self.filters_open, |d| d.child(self.filters_menu(cx)))
            .when(self.connections_open, |d| d.child(self.connection_menu(cx)))
            .when(self.project_picker, |d| d.child(self.project_menu(cx)))
            .when(self.action_menu, |d| d.child(self.action_menu_view(cx)))
            .when(self.connect_provider.is_some(), |d| {
                d.child(self.connection_dialog(cx))
            })
            .when(self.confirm_action.is_some(), |d| {
                d.child(self.action_confirmation(cx))
            })
    }
}

mod detail;
mod menus;
mod summary;
#[cfg(test)]
mod tests;
