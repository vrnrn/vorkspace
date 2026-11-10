use super::*;
use crate::controls::page;
use crate::popover::{Align, Anchor, popover};
use gpui::{
    AnyElement, ClipboardItem, Div, FontWeight, Hsla, MouseButton, Pixels, Role, SharedString,
    Stateful, Window, canvas, div, list, prelude::*, px, rgb,
};
use vorkspace_ui::radius;

impl Inbox {
    fn effective_width(&self, window: &Window) -> f32 {
        self.pane_width
            .unwrap_or_else(|| f32::from(window.viewport_size().width))
    }

    fn is_narrow(&self, window: &Window) -> bool {
        self.pane_width
            .map_or_else(|| page::narrow(window), |width| width < 900.)
    }

    fn detail_inset(&self, window: &Window) -> Pixels {
        page::detail_padding(self.is_narrow(window))
    }
}

fn action(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Stateful<Div> {
    action_enabled(id, label, true)
}
fn action_enabled(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    enabled: bool,
) -> Stateful<Div> {
    let id: SharedString = id.into();
    let label = label.into();
    page::action(id.clone(), label.clone(), enabled)
        .debug_selector(move || id.to_string())
        .max_w_full()
        .child(div().min_w_0().truncate().child(label))
}
fn icon_button(id: &'static str, label: &'static str, name: &'static str) -> Stateful<Div> {
    page::icon_action(id, label, true).child(action_icon(name, page::secondary_ink()))
}
fn icon_action(
    id: impl Into<SharedString>,
    label: &str,
    name: &'static str,
    enabled: bool,
) -> Stateful<Div> {
    let id: SharedString = id.into();
    page::action(id.clone(), label.to_owned(), enabled)
        .debug_selector(move || id.to_string())
        .max_w_full()
        .child(action_icon(name, page::secondary_ink()))
        .child(div().min_w_0().truncate().child(label.to_owned()))
}
fn action_icon(name: &'static str, ink: Hsla) -> gpui::Svg {
    icon(name, page::ICON_SIZE).text_color(ink)
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
    page::meta()
        .h(px(20.))
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(6.))
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
    page::action(id.clone(), label.clone(), true)
        .debug_selector(move || id.to_string())
        .role(Role::MenuItemCheckBox)
        .aria_toggled(if checked {
            gpui::Toggled::True
        } else {
            gpui::Toggled::False
        })
        .justify_start()
        .gap(px(8.))
        .child(
            div()
                .size(px(14.))
                .when(checked, |d| d.child(icon("check", 14.))),
        )
        .child(div().flex_1().min_w_0().truncate().child(label))
}
fn caption(text: impl Into<SharedString>) -> Div {
    page::meta()
        .px(px(8.))
        .pt(px(8.))
        .pb(px(4.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}
fn message(text: impl Into<SharedString>) -> Div {
    page::meta().p(px(12.)).child(text.into())
}
fn error(text: impl Into<SharedString>) -> Div {
    message(text).text_color(status(StatusColor::Danger, 1.))
}
fn provider_mark(provider: &str) -> gpui::Svg {
    let (path, color) = match provider {
        "jira" => ("inbox-jira", rgb(0x2684ff).into()),
        "gitlab" => ("inbox-gitlab", rgb(0xfc6d26).into()),
        _ => ("inbox-github", content(0.8)),
    };
    icon(path, page::ICON_SIZE).text_color(color)
}
fn status_color(item: &Item) -> Hsla {
    match item.status() {
        "Merged" => rgb(if is_light() { 0x7146b8 } else { 0xa78bfa }).into(),
        "Closed" => status(StatusColor::Danger, 1.),
        "Draft" => page::secondary_ink(),
        _ => status(StatusColor::Success, 1.),
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
    page::meta()
        .max_w(px(if compact { 80. } else { 200. }))
        .rounded(px(radius::CONTROL))
        .px(px(4.))
        .flex()
        .items_center()
        .gap(px(4.))
        .bg(content(0.08))
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
    /// A disabled or unconnected source has no items to open, so its page is
    /// only the explanation and the way to connect, across the full width.
    fn unavailable(&self) -> bool {
        let source = &self.state.source;
        if self
            .snapshot
            .items
            .iter()
            .any(|item| &item.provider == source)
        {
            return false;
        }
        !self.state.connectors.enabled(source)
            || !self
                .snapshot
                .connections
                .get(&self.state.source)
                .is_some_and(|status| status.connected)
    }

    fn list_view(&self, narrow: bool, cx: &mut Context<Self>) -> AnyElement {
        let full = narrow || self.unavailable();
        let provider = model::provider_label(&self.state.source);
        let connected = self
            .snapshot
            .connections
            .get(&self.state.source)
            .is_some_and(|status| status.connected);
        let mut items = self.visible_items(cx).peekable();
        let tools = page::toolbar()
            .when(!self.unavailable(), |d| {
                d.child(
                    page::search_frame(&self.search.focus_handle(cx))
                        .flex_1()
                        .child(action_icon("search", page::secondary_ink()))
                        .child(self.search.clone()),
                )
            })
            .when(self.unavailable(), |d| d.child(div().flex_1()))
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
        let (empty_title, empty_detail) = if !self.state.connectors.enabled(&self.state.source) {
            (
                format!("{provider} is disabled"),
                "Turn it on in Settings → Plugins.".into(),
            )
        } else if let Some(error) = self.snapshot.errors.get(&self.state.source) {
            (
                format!("Could not load {provider}"),
                format!("{error}. Check Settings → {provider} or refresh."),
            )
        } else if self.loading && items.peek().is_none() {
            (format!("Loading {provider}…"), String::new())
        } else if !connected {
            (
                format!("{provider} isn't connected"),
                "Connect your account to see its items here.".into(),
            )
        } else if self.projects.is_empty() && !model::is_tracker(&self.state.source) {
            (
                String::from("No projects open"),
                format!("Open a project to see its {provider} items."),
            )
        } else if self.filters_active() || !self.search.read(cx).content.is_empty() {
            (
                format!("No matching {provider} items"),
                "Try a different search or clear your filters.".into(),
            )
        } else {
            (format!("No {provider} items"), String::new())
        };
        let rows = items
            .map(|item| {
                let key = item.key();
                let active = self.selected.as_ref() == Some(&key);
                let unread = self.state.seen.unseen(item);
                let sessions = self.sessions.matching(item).count();
                let project = self.scoped_project(item).unwrap_or(&item.project_path);
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
                    project_name(std::path::Path::new(project))
                };
                let attention = attention_label(&item.attention_reason);
                let row_id = format!("inbox-item-{key}");
                let middle_key = key.clone();
                page::row(row_id.clone(), item.title.clone(), active)
                    .debug_selector(move || row_id.clone())
                    .role(Role::ListBoxOption)
                    .aria_selected(active)
                    .child(
                        page::meta()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(provider_mark(&item.provider))
                            .child(icon(status_icon(item), 12.).text_color(status_color(item)))
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
                            .child(relative_time(&item.updated_at))
                            .when(unread, |d| {
                                d.child(div().size(px(6.)).rounded_full().bg(accent_color()))
                            }),
                    )
                    .child(
                        page::label()
                            .mt(px(4.))
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(item.title.clone()),
                    )
                    .child(
                        div()
                            .mt(px(4.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                page::meta()
                                    .min_w_0()
                                    .flex_1()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .when(!tracker && !project.is_empty(), |d| {
                                        d.child(crate::project_mark::project_icon(project, 12.))
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
                    .on_mouse_up(
                        MouseButton::Middle,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            if let Some(item) = this.item_in_list(&middle_key) {
                                cx.emit(Event::OpenTab(Box::new(item)));
                            }
                        }),
                    )
                    .on_click(
                        cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                            let modifiers = event.modifiers();
                            if modifiers.platform || modifiers.control {
                                if let Some(item) = this.item_in_list(&key) {
                                    cx.emit(Event::OpenTab(Box::new(item)));
                                }
                                cx.stop_propagation();
                                return;
                            }
                            this.narrow_detail = true;
                            this.select(key.clone(), cx);
                            if this.is_narrow(window) {
                                window.focus(&this.focus, cx);
                                cx.notify();
                            }
                        }),
                    )
            })
            .collect::<Vec<_>>();
        div()
            .relative()
            .when(full, |d| d.w_full())
            .when(!full, |d| {
                d.w(px(self
                    .state
                    .list_width
                    .clamp(page::LIST_MIN, page::LIST_MAX)))
            })
            .h_full()
            .flex_shrink_0()
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
                    .p(px(4.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .when(rows.is_empty(), |d| {
                        let source = self.state.source.clone();
                        let can_connect = !connected
                            && self
                                .state
                                .connectors
                                .enabled_providers()
                                .any(|p| *p == source);
                        d.child(
                            page::empty(
                                provider_mark(&self.state.source)
                                    .size(px(24.))
                                    .text_color(page::secondary_ink()),
                                empty_title,
                                empty_detail,
                            )
                            // The way out of an unconnected page is one click.
                            .when(can_connect, |d| {
                                let label = format!("Connect {provider}");
                                d.child(
                                    crate::controls::primary_button(
                                        "inbox-empty-connect",
                                        label.clone(),
                                        false,
                                    )
                                    .debug_selector(|| "inbox-empty-connect".into())
                                    .mt(px(12.))
                                    .px(px(12.))
                                    .child(label)
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.open_connection(source.clone(), cx)
                                        },
                                    )),
                                )
                            }),
                        )
                    })
                    .children(rows),
            )
            .when(!full, |d| {
                d.child(
                    page::detail_resize_handle(
                        "inbox-list-resize",
                        "Resize connector list",
                        self.resize_start.is_some(),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                            this.resize_start =
                                Some((f32::from(event.position.x), this.state.list_width));
                            cx.stop_propagation();
                        }),
                    ),
                )
            })
            .into_any_element()
    }

    fn back_to_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.narrow_detail = false;
        window.focus(&self.search.focus_handle(cx), cx);
        cx.notify();
    }
}

impl gpui::Render for Inbox {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let narrow = self.is_narrow(window);
        let pinned = self.pinned_item.is_some();
        let showing_detail = pinned || (narrow && self.narrow_detail && self.item().is_some());
        if self.focus_comment && (!narrow || showing_detail) {
            self.focus_comment = false;
            window.focus(&self.comment.focus_handle(cx), cx);
        }
        div()
            .id("inbox-view")
            .role(Role::Group)
            .aria_label(model::provider_label(&self.state.source))
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    if this.connect_provider.is_some() && !this.connecting {
                        this.dismiss_connection(cx);
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
                    } else if this.pinned_item.is_none()
                        && this.is_narrow(window)
                        && this.narrow_detail
                        && this.item().is_some()
                    {
                        this.back_to_list(window, cx);
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
                        let max = (this.effective_width(window) * 0.5)
                            .clamp(page::LIST_MIN, page::LIST_MAX);
                        this.state.list_width = (width + f32::from(event.position.x) - start)
                            .clamp(page::LIST_MIN, max);
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
                    .when(!pinned && (!narrow || !showing_detail), |d| {
                        d.child(self.list_view(narrow, cx))
                    })
                    .when(
                        pinned || ((!narrow || showing_detail) && !self.unavailable()),
                        |d| {
                            d.child(
                                page::detail_pane(!narrow && !pinned)
                                    .when(narrow && !pinned, |d| {
                                        d.child(
                                            page::toolbar().child(
                                                icon_action(
                                                    "inbox-back",
                                                    "Back to items",
                                                    "arrow-left",
                                                    true,
                                                )
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.back_to_list(window, cx);
                                                })),
                                            ),
                                        )
                                    })
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_h_0()
                                            .min_w_0()
                                            .child(self.detail_view(window, cx)),
                                    ),
                            )
                        },
                    ),
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
#[cfg(test)]
mod screenshots;
mod summary;
#[cfg(test)]
mod tests;
