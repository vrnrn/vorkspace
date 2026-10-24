//! Pending approval/question cards and request-level announcement bookkeeping.
use crate::theme::*;
use gpui::{Context, EventEmitter, Role, div, prelude::*, px};
use std::{
    collections::{HashMap, HashSet},
    time::SystemTime,
};
use vorkspace_core::agent::Decision;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum InputKind {
    Approval,
    Question,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingInput {
    pub request_id: String,
    pub kind: InputKind,
    pub label: String,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SessionInputs {
    pub session_id: String,
    pub title: String,
    /// Provider key accepted by `theme::provider_icon`.
    pub provider: String,
    pub harness: String,
    /// Approvals in transcript order, followed by the pending question(s).
    pub pending: Vec<PendingInput>,
    pub inbox_ask: bool,
    pub orchestration_worker: bool,
    /// Active tab, focused pane, and focused composer must all match.
    pub conversation_focused: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct InputKey {
    pub session_id: String,
    pub kind: InputKind,
    pub request_id: String,
}
impl SessionInputs {
    fn key(&self, input: &PendingInput) -> InputKey {
        InputKey {
            session_id: self.session_id.clone(),
            kind: input.kind,
            request_id: input.request_id.clone(),
        }
    }
    fn notice(&self) -> Option<&PendingInput> {
        self.pending
            .iter()
            .find(|p| p.kind == InputKind::Question)
            .or_else(|| {
                self.pending
                    .iter()
                    .rev()
                    .find(|p| p.kind == InputKind::Approval)
            })
    }
}

pub(crate) enum Action {
    OpenSession {
        session_id: String,
    },
    Decide {
        session_id: String,
        request_id: String,
        decision: Decision,
    },
}

/// Mirrors useInputNotifications: coalesce each update per session, retain
/// unannounced requests, and forget resolved keys. Mark before policy checks.
#[derive(Default)]
pub(crate) struct InputAnnouncements {
    notified: HashSet<InputKey>,
}
impl InputAnnouncements {
    pub(crate) fn reconcile(
        &mut self,
        sessions: &[SessionInputs],
    ) -> Vec<(SessionInputs, PendingInput)> {
        let pending: HashSet<_> = sessions
            .iter()
            .filter(|s| !s.inbox_ask)
            .flat_map(|s| s.pending.iter().map(|p| s.key(p)))
            .collect();
        self.notified.retain(|key| pending.contains(key));
        let mut announcements = Vec::new();
        for session in sessions.iter().filter(|s| !s.inbox_ask) {
            if let Some(input) = session
                .pending
                .iter()
                .find(|p| !self.notified.contains(&session.key(p)))
            {
                self.notified.insert(session.key(input));
                announcements.push((session.clone(), input.clone()));
            }
        }
        announcements
    }
}

#[derive(Default)]
pub(crate) struct ApprovalNotices {
    notices: Vec<(SessionInputs, PendingInput)>,
    occurred: HashMap<InputKey, SystemTime>,
}
impl EventEmitter<Action> for ApprovalNotices {}
impl ApprovalNotices {
    /// Observe pending requests from session events even while the window is
    /// occluded. Policy must retain this first observation until resolution.
    pub(crate) fn track_pending(&mut self, sessions: &[SessionInputs]) {
        let now = SystemTime::now();
        let pending: HashSet<_> = sessions
            .iter()
            .flat_map(|session| session.pending.iter().map(|input| session.key(input)))
            .collect();
        self.occurred.retain(|key, _| pending.contains(key));
        for key in pending {
            self.occurred.entry(key).or_insert(now);
        }
    }
    /// `allows` must apply known-project + agentInput project policy using the
    /// original occurrence timestamp, so resuming cannot revive muted requests.
    pub(crate) fn sync(
        &mut self,
        sessions: &[SessionInputs],
        mut allows: impl FnMut(&SessionInputs, SystemTime) -> bool,
        cx: &mut Context<Self>,
    ) {
        let previous = self.notices.clone();
        self.track_pending(sessions);
        self.notices.clear();
        for session in sessions {
            if session.inbox_ask || session.orchestration_worker || session.conversation_focused {
                continue;
            }
            let Some(input) = session.notice() else {
                continue;
            };
            let key = session.key(input);
            let occurred = *self.occurred.entry(key).or_insert_with(SystemTime::now);
            if allows(session, occurred) {
                self.notices.push((session.clone(), input.clone()));
            }
        }
        if self.notices != previous {
            cx.notify();
        }
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.notices.is_empty()
    }
}
impl Render for ApprovalNotices {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .w_full()
            .children(
                self.notices
                    .iter()
                    .enumerate()
                    .map(|(ix, (session, input))| {
                        let open_id = session.session_id.clone();
                        let allow_session = session.session_id.clone();
                        let deny_session = session.session_id.clone();
                        let allow_request = input.request_id.clone();
                        let deny_request = input.request_id.clone();
                        super::surface::toast_frame()
                            .id(("approval-toast", ix))
                            .role(Role::Status)
                            .child(
                                div()
                                    .id(("approval-toast-open", ix))
                                    .role(Role::Button)
                                    .cursor_pointer()
                                    .px(px(14.))
                                    .py_3()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .hover(|s| s.bg(content(0.05)))
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(Action::OpenSession {
                                            session_id: open_id.clone(),
                                        })
                                    }))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(provider_icon(&session.provider, 16.))
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .flex_1()
                                                    .truncate()
                                                    .text_size(px(13.))
                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                                    .child(session.title.clone()),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_1()
                                                    .text_size(px(11.))
                                                    .text_color(gpui::rgb(0xffb900))
                                                    .child(
                                                        icon("circle-alert", 14.)
                                                            .text_color(gpui::rgb(0xffb900)),
                                                    )
                                                    .child(if input.kind == InputKind::Question {
                                                        "Question"
                                                    } else {
                                                        "Approval"
                                                    }),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .line_height(px(19.5))
                                            .line_clamp(3)
                                            .text_color(content(0.7))
                                            .child(input.label.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(content(0.4))
                                            .child(session.harness.clone()),
                                    ),
                            )
                            .when(input.kind == InputKind::Approval, |card| {
                                card.child(
                                    div()
                                        .flex()
                                        .gap_2()
                                        .border_t_1()
                                        .border_color(content(0.1))
                                        .px(px(14.))
                                        .py(px(10.))
                                        .child(
                                            div()
                                                .id(("approval-toast-allow", ix))
                                                .role(Role::Button)
                                                .aria_label("Allow")
                                                .cursor_pointer()
                                                .flex_1()
                                                .rounded_md()
                                                .px(px(10.))
                                                .py_1()
                                                .text_center()
                                                .text_size(px(11.))
                                                .bg(content(1.))
                                                .text_color({
                                                    let mut color = pane_fill();
                                                    color.a = 1.;
                                                    color
                                                })
                                                .hover(|s| s.bg(content(0.8)))
                                                .child("Allow")
                                                .on_click(cx.listener(move |_, _, _, cx| {
                                                    cx.emit(Action::Decide {
                                                        session_id: allow_session.clone(),
                                                        request_id: allow_request.clone(),
                                                        decision: Decision::Allow,
                                                    })
                                                })),
                                        )
                                        .child(
                                            div()
                                                .id(("approval-toast-deny", ix))
                                                .role(Role::Button)
                                                .aria_label("Deny")
                                                .cursor_pointer()
                                                .flex_1()
                                                .rounded_md()
                                                .px(px(10.))
                                                .py_1()
                                                .text_center()
                                                .text_size(px(11.))
                                                .bg(content(0.1))
                                                .text_color(content(0.7))
                                                .hover(|s| s.bg(content(0.2)))
                                                .child("Deny")
                                                .on_click(cx.listener(move |_, _, _, cx| {
                                                    cx.emit(Action::Decide {
                                                        session_id: deny_session.clone(),
                                                        request_id: deny_request.clone(),
                                                        decision: Decision::Deny,
                                                    })
                                                })),
                                        ),
                                )
                            })
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use std::{cell::Cell, rc::Rc};

    #[gpui::test]
    fn sync_notifies_only_when_visible_approval_notices_change(cx: &mut TestAppContext) {
        let notices = cx.new(|_| ApprovalNotices::default());
        let count = Rc::new(Cell::new(0));
        let _observe = cx.update(|cx| {
            let count = count.clone();
            cx.observe(&notices, move |_, _| count.set(count.get() + 1))
        });
        let sessions = [SessionInputs {
            session_id: "session".into(),
            title: "Session".into(),
            provider: "provider".into(),
            harness: "harness".into(),
            pending: vec![PendingInput {
                request_id: "request".into(),
                kind: InputKind::Approval,
                label: "Approve".into(),
            }],
            inbox_ask: false,
            orchestration_worker: false,
            conversation_focused: false,
        }];
        notices.update(cx, |notices, cx| {
            notices.sync(&sessions, |_, _| true, cx);
        });
        cx.run_until_parked();
        assert_eq!(count.get(), 1);
        notices.update(cx, |notices, cx| {
            notices.sync(&sessions, |_, _| true, cx);
        });
        cx.run_until_parked();
        assert_eq!(count.get(), 1);
        notices.update(cx, |notices, cx| {
            notices.sync(&[], |_, _| true, cx);
            assert!(notices.is_empty());
        });
        cx.run_until_parked();
        assert_eq!(count.get(), 2);
    }
}
