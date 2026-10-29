use crate::{
    popover::{Anchor, AnchorSlot, Side, popover},
    theme::*,
};
use gpui::{
    App, Context, FocusHandle, FontWeight, KeyBinding, Role, Window, actions, div, prelude::*, px,
    rgb,
};
use std::time::{Duration, Instant};
#[cfg(test)]
use vorkspace_core::usage::{FetchError, Snapshot};
use vorkspace_core::{provider::Kind, usage};

mod resets;

actions!(usage_footer, [DismissUsage, ToggleUsage]);
pub(crate) fn bind(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", DismissUsage, Some("UsageDetails")),
        KeyBinding::new("enter", ToggleUsage, Some("UsageChip")),
        KeyBinding::new("space", ToggleUsage, Some("UsageChip")),
    ]);
}

use crate::accounts::{Cached as CachedUsage, Key};

fn index(kind: Kind) -> usize {
    match kind {
        Kind::Codex => 0,
        Kind::Claude => 1,
        _ => {
            2 + Kind::ALL
                .iter()
                .filter(|k| !matches!(k, Kind::Codex | Kind::Claude))
                .position(|k| *k == kind)
                .unwrap()
        }
    }
}

/// Its own cached surface: clock ticks and probe completions don't rebuild chat.
pub(crate) struct UsageFooter {
    kind: Kind,
    key: Key,
    project: String,
    session: Option<i64>,
    show_accounts: bool,
    confirm_reset: Option<String>,
    _accounts_observation: Option<gpui::Subscription>,
    _account_events: Option<gpui::Subscription>,
    cache: [CachedUsage; Kind::ALL.len()],
    anchor: AnchorSlot,
    trigger: FocusHandle,
    refresh_focus: FocusHandle,
    #[cfg(test)]
    refresh_requests: Vec<Key>,
    details: FocusHandle,
    previous_focus: Option<FocusHandle>,
    open: bool,
    compact: bool,
    context_key: Key,
    rows: std::collections::HashMap<Key, (AnchorSlot, FocusHandle, FocusHandle)>,
    show_remaining: bool,
    _clock: gpui::Task<()>,
}
impl UsageFooter {
    pub(crate) fn context(
        &mut self,
        kind: Kind,
        profile: vorkspace_core::accounts::Profile,
        model: Option<&str>,
        project: String,
        session: Option<i64>,
        cx: &mut Context<Self>,
    ) {
        let key = Key::new(kind, profile, model);
        self.project = project;
        self.session = session;
        if self.context_key != key {
            self.context_key = key.clone();
            self.kind = kind;
            self.select_subscription(key, cx);
            self.open = false;
            self.show_accounts = false;
            self.confirm_reset = None;
            self.cache[index(kind)] = Default::default();
            self.refresh(false, cx);
            cx.notify();
        }
    }
    pub(crate) fn new(kind: Kind, cx: &mut Context<Self>) -> Self {
        let clock = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(30))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        this.refresh(false, cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let observation = crate::accounts::get(cx).map(|center| {
            cx.observe(&center, |this, center, cx| {
                if let Some(state) = center.read(cx).usage.get(&this.key).cloned() {
                    this.cache[index(this.kind)] = state;
                }
                cx.notify();
            })
        });
        let mut this = Self {
            _account_events: crate::accounts::get(cx).map(|center| {
                cx.subscribe(
                    &center,
                    |this, center, event: &crate::accounts::AccountEvent, cx| {
                        if matches!(
                            event,
                            crate::accounts::AccountEvent::Updated
                                | crate::accounts::AccountEvent::Authenticated(..)
                        ) {
                            this.refresh_subscriptions(cx);
                        }
                        let modal_auth =
                            matches!(event, crate::accounts::AccountEvent::AuthStarted)
                                && center.read(cx).has_modal_auth();
                        if modal_auth
                            || matches!(event, crate::accounts::AccountEvent::Removing(kind, id) if *kind == this.kind && *id == this.key.profile.id)
                            || matches!(
                                event,
                                crate::accounts::AccountEvent::Select { .. }
                                    | crate::accounts::AccountEvent::PickerDismiss
                                    | crate::accounts::AccountEvent::Manage
                            )
                        {
                            cx.spawn(async move |this, cx| {
                                let _ =
                                    this.update_in(cx, |this, window, cx| this.dismiss(window, cx));
                            })
                            .detach();
                        }
                    },
                )
            }),
            kind,
            key: Key::new(kind, Default::default(), None),
            project: String::new(),
            session: None,
            show_accounts: false,
            confirm_reset: None,
            _accounts_observation: observation,
            cache: Default::default(),
            anchor: AnchorSlot::default(),
            trigger: cx.focus_handle(),
            refresh_focus: cx.focus_handle(),
            #[cfg(test)]
            refresh_requests: Vec::new(),
            details: cx.focus_handle(),
            previous_focus: None,
            open: false,
            compact: false,
            context_key: Key::new(kind, Default::default(), None),
            rows: Default::default(),
            show_remaining: false,
            _clock: clock,
        };
        this.rows.insert(
            this.key.clone(),
            (
                this.anchor.clone(),
                this.trigger.clone(),
                this.refresh_focus.clone(),
            ),
        );
        this.refresh_subscriptions(cx);
        this.refresh(false, cx);
        this
    }
    pub(crate) fn set_remaining(&mut self, remaining: bool, cx: &mut Context<Self>) {
        if self.show_remaining != remaining {
            self.show_remaining = remaining;
            cx.notify();
        }
    }
    fn display_percent(&self, used: f64) -> f64 {
        if self.show_remaining {
            100. - used
        } else {
            used
        }
    }
    fn percent_label(&self, used: f64) -> String {
        format!(
            "{:.0}% {}",
            self.display_percent(used),
            if self.show_remaining { "left" } else { "used" }
        )
    }
    pub(crate) fn hide(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        self.show_accounts = false;
        self.confirm_reset = None;
        self.previous_focus = None;
        cx.notify();
    }
    pub(crate) fn set_compact(&mut self, compact: bool, cx: &mut Context<Self>) {
        if self.compact != compact {
            self.compact = compact;
            cx.notify();
        }
    }
    /// Clicking an account refreshes that account's usage in addition to
    /// opening its details, so no separate refresh control is needed.
    fn request_refresh(&mut self, key: Key, cx: &mut Context<Self>) {
        #[cfg(test)]
        self.refresh_requests.push(key.clone());
        if let Some(center) = crate::accounts::get(cx) {
            center.update(cx, |center, cx| center.refresh(key, true, cx));
        } else {
            self.refresh(true, cx);
        }
    }
    fn refresh_subscriptions(&self, cx: &mut Context<Self>) {
        if let Some(center) = crate::accounts::get(cx) {
            center.update(cx, |center, cx| {
                for kind in vorkspace_core::accounts::PROVIDERS {
                    center.load_provider(kind, false, cx);
                }
            });
        }
    }
    fn select_subscription(&mut self, key: Key, cx: &mut Context<Self>) {
        let chrome = self
            .rows
            .entry(key.clone())
            .or_insert_with(|| (AnchorSlot::default(), cx.focus_handle(), cx.focus_handle()));
        self.anchor = chrome.0.clone();
        self.trigger = chrome.1.clone();
        self.refresh_focus = chrome.2.clone();
        self.kind = key.kind;
        self.key = key;
        self.cache[index(self.kind)] = crate::accounts::get(cx)
            .and_then(|center| center.read(cx).usage.get(&self.key).cloned())
            .unwrap_or_default();
    }
    fn open_subscription(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) {
        // Bedrock bills outside the provider, so there is no usage to show:
        // go to the user's own cost monitoring page when they set one.
        let bedrock = crate::accounts::get(cx).is_some_and(|center| {
            center
                .read(cx)
                .usage
                .get(&key)
                .and_then(|cached| cached.snapshot.as_ref())
                .is_some_and(|s| s.external_billing.as_deref() == Some("AWS Bedrock"))
        });
        if bedrock
            && let Some(url) = cx
                .try_global::<crate::settings::Preferences>()
                .and_then(|p| p.cost_link().map(str::to_owned))
        {
            cx.open_url(&url);
            return;
        }
        if self.key != key {
            if self.open {
                self.dismiss(window, cx);
            }
            self.select_subscription(key, cx);
        }
        self.toggle(window, cx);
    }
    fn subscriptions(&self, cx: &App) -> Vec<(Key, String, CachedUsage)> {
        let Some(center) = crate::accounts::get(cx) else {
            return vec![(
                self.key.clone(),
                self.kind.label().into(),
                self.cache[index(self.kind)].clone(),
            )];
        };
        let center = center.read(cx);
        let mut keys: Vec<_> = vorkspace_core::accounts::PROVIDERS
            .into_iter()
            .flat_map(|kind| {
                center
                    .book
                    .list(kind)
                    .into_iter()
                    .map(move |account| Key::new(kind, center.profile(&account.id), None))
            })
            .collect();
        keys.extend(
            center
                .usage
                .keys()
                .filter(|key| !keys.contains(key))
                .cloned()
                .collect::<Vec<_>>(),
        );
        keys.sort_by_key(|key| (index(key.kind), key.profile.id.clone(), key.billing.clone()));
        let mut rows: Vec<_> = keys
            .into_iter()
            .filter_map(|key| {
                if key.profile.id != vorkspace_core::accounts::DEFAULT
                    && !center.book.exists(key.kind, &key.profile.id)
                {
                    return None;
                }
                let state = center.usage.get(&key).cloned().unwrap_or_default();
                let named = key.profile.id != vorkspace_core::accounts::DEFAULT;
                if !named
                    && state.snapshot.is_none()
                    && !state.fetching
                    && state.error.as_ref().is_none_or(|error| error.unavailable)
                    && !(self.open && key == self.key)
                {
                    return None;
                }
                let account = center
                    .book
                    .list(key.kind)
                    .into_iter()
                    .find(|account| account.id == key.profile.id);
                let label = if key.billing.is_empty() {
                    let multiple = center.book.list(key.kind).len() > 1;
                    match account.filter(|account| multiple || account.label != "Default account") {
                        Some(account) => format!("{} · {}", key.kind.label(), account.label),
                        None => key.kind.label().into(),
                    }
                } else {
                    external_billing(state.snapshot.as_ref())
                        .map(str::to_owned)
                        .unwrap_or_else(|| {
                            format!(
                                "{} · {}",
                                key.kind.label(),
                                match key.billing.as_str() {
                                    "anthropic" => "Claude",
                                    "openai-codex" => "Codex",
                                    billing => billing,
                                }
                            )
                        })
                };
                Some((key, label, state))
            })
            .collect();
        if rows.is_empty() {
            rows.push((
                self.context_key.clone(),
                self.context_key.kind.label().into(),
                center
                    .usage
                    .get(&self.context_key)
                    .cloned()
                    .unwrap_or_default(),
            ));
        }
        rows
    }
    #[cfg(test)]
    pub(crate) fn provider(&mut self, kind: Kind, cx: &mut Context<Self>) {
        if self.kind != kind {
            self.kind = kind;
            self.key = Key::new(kind, Default::default(), None);
            self.open = false;
            self.refresh(false, cx);
            cx.notify();
        }
    }
    fn refresh(&mut self, force: bool, cx: &mut Context<Self>) {
        if let Some(center) = crate::accounts::get(cx) {
            let key = self.key.clone();
            center.update(cx, |center, cx| center.refresh(key.clone(), force, cx));
            if let Some(state) = center.read(cx).usage.get(&key).cloned() {
                self.cache[index(self.kind)] = state;
            }
            cx.notify();
            return;
        }
        if cfg!(test) {
            return;
        }
        let slot = index(self.kind);
        let state = &mut self.cache[slot];
        let reset = state.snapshot.as_ref().is_some_and(|s| {
            s.windows.iter().any(|w| {
                w.resets_at
                    .is_some_and(|at| at > s.updated_at && at <= usage::now_ms())
            })
        });
        let interval = if reset { 300 } else { 900 };
        if state.fetching
            || (!force
                && state
                    .attempted
                    .is_some_and(|at| at.elapsed() < Duration::from_secs(interval)))
        {
            return;
        }
        state.fetching = true;
        state.attempted = Some(Instant::now());
        let kind = self.kind;
        let work = cx
            .background_executor()
            .spawn(async move { usage::fetch(kind) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                // Store by the requested provider, even if the user changed tabs.
                this.cache[slot].complete(result);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    #[cfg(test)]
    pub(crate) fn kind(&self) -> Kind {
        self.kind
    }
    #[cfg(test)]
    pub(crate) fn subscription_chrome(&self, key: &Key) -> (AnchorSlot, FocusHandle) {
        let row = &self.rows[key];
        (row.0.clone(), row.1.clone())
    }
    #[cfg(test)]
    pub(crate) fn open_subscription_key(&self) -> Option<&Key> {
        self.open.then_some(&self.key)
    }
    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.dismiss(window, cx);
        } else {
            self.previous_focus = window.focused(cx).filter(|f| *f != self.trigger);
            self.open = true;
            self.show_accounts = false;
            if let Some(center) = crate::accounts::get(cx) {
                let kind = self.kind;
                let key = self.key.clone();
                center.update(cx, |center, cx| {
                    if center
                        .reset_actions
                        .get(&key)
                        .is_some_and(|a| !a.applying() && a.row.is_none())
                    {
                        center.reset_actions.remove(&key);
                    }
                    center.hide_emails();
                    center.load_provider(kind, false, cx);
                });
            }
            window.focus(&self.details, cx);
            cx.notify();
        }
    }
    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        self.confirm_reset = None;
        window.focus(self.previous_focus.as_ref().unwrap_or(&self.trigger), cx);
        cx.notify();
    }
    fn login_view(&self, cx: &App) -> bool {
        let state = &self.cache[index(self.kind)];
        if external_billing(state.snapshot.as_ref()).is_some() {
            return false;
        }
        vorkspace_core::accounts::login_arguments(
            self.kind,
            vorkspace_core::accounts::AuthAction::SignIn,
        )
        .is_some()
            && state.snapshot.as_ref().is_none_or(|s| s.windows.is_empty())
            && (state.error.as_ref().is_some_and(|e| e.unavailable)
                || crate::accounts::get(cx).is_some_and(|center| {
                    center
                        .read(cx)
                        .has_footer_auth(self.kind, &self.key.profile.id)
                }))
    }
    /// React's AccountSwitchRow, above the sign-in panel.
    fn account_row(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let label = crate::accounts::get(cx)
            .and_then(|center| {
                center
                    .read(cx)
                    .book
                    .list(self.kind)
                    .into_iter()
                    .find(|a| a.id == self.key.profile.id)
            })
            .map(|a| a.label)
            .unwrap_or_else(|| "Removed account".into());
        div()
            .id("usage-switch-account")
            .role(Role::Button)
            .aria_label(format!("Switch account from {label}"))
            .h_8()
            .px(px(10.))
            .rounded_lg()
            .bg(content(0.045))
            .border_1()
            .border_color(content(0.06))
            .hover(|s| s.bg(content(0.08)))
            .cursor_pointer()
            .flex()
            .items_center()
            .gap_2()
            .text_size(px(11.))
            .child(div().flex_1().min_w_0().truncate().child(label))
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(content(0.4))
                    .child("Switch"),
            )
            .child(icon("chevron-right", 12.).text_color(content(0.35)))
            .on_click(cx.listener(|this, _, window, cx| this.open_accounts(window, cx)))
            .into_any_element()
    }
    fn details(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        if self.show_accounts
            && let Some(center) = crate::accounts::get(cx)
        {
            let kind = self.kind;
            let selected = self.key.profile.id.clone();
            let project = self.project.clone();
            let session = self.session.filter(|_| self.kind == self.context_key.kind);
            let weak = cx.weak_entity();
            let picker = center.update(cx, |center, cx| {
                center.picker(
                    kind,
                    &selected,
                    &project,
                    session,
                    move |window, cx| {
                        let _ = weak.update(cx, |this, cx| {
                            this.show_accounts = false;
                            window.focus(&this.details, cx);
                            cx.notify();
                        });
                    },
                    cx,
                )
            });
            return div()
                .id("footer-account-picker")
                .role(Role::Dialog)
                .aria_label("Choose account")
                .track_focus(&self.details)
                .key_context("UsageDetails")
                .on_action(
                    cx.listener(|this, _: &DismissUsage, window, cx| this.dismiss(window, cx)),
                )
                .child(picker)
                .into_any_element();
        }

        if self.login_view(cx)
            && let Some(center) = crate::accounts::get(cx)
        {
            let panel = center.update(cx, |center, cx| {
                center.footer_auth_panel(self.kind, &self.key.profile.id, cx)
            });
            return div()
                .id("usage-details-content")
                .role(Role::Dialog)
                .aria_label(format!("{} usage details", self.kind.label()))
                .track_focus(&self.details)
                .key_context("UsageDetails")
                .on_action(
                    cx.listener(|this, _: &DismissUsage, window, cx| this.dismiss(window, cx)),
                )
                .flex()
                .flex_col()
                .when(
                    vorkspace_core::accounts::supports_accounts(self.kind),
                    |d| d.child(div().px(px(10.)).pt(px(10.)).child(self.account_row(cx))),
                )
                .child(panel)
                .into_any_element();
        }
        let state = &self.cache[index(self.kind)];
        let now = usage::now_ms();
        let updated = match &state.snapshot {
            Some(snapshot) if snapshot.updated_at > 0 => {
                match (now - snapshot.updated_at).max(0) / 60_000 {
                    0 => "Updated just now".to_owned(),
                    minutes @ 1..60 => format!("Updated {minutes}m ago"),
                    minutes => format!("Updated {}h ago", minutes / 60),
                }
            }
            _ => "Rate-limit details".into(),
        };
        let windows = state.snapshot.iter().flat_map(|s| &s.windows);
        let has_windows = state
            .snapshot
            .as_ref()
            .is_some_and(|s| !s.windows.is_empty());
        div()
            .id("usage-details-content")
            .role(Role::Dialog)
            .aria_label(format!("{} usage details", self.kind.label()))
            .track_focus(&self.details)
            .key_context("UsageDetails")
            .on_action(cx.listener(|this, _: &DismissUsage, window, cx| {
                this.dismiss(window, cx);
            }))
            .flex()
            .flex_col()
            .text_size(px(11.))
            .text_color(content(1.))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(10.))
                    .px_1()
                    .pt(px(2.))
                    .pb(px(10.))
                    .child(
                        div()
                            .size_7()
                            .flex_shrink_0()
                            .rounded_lg()
                            .bg(content(0.06))
                            .border_1()
                            .border_color(content(0.07))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(provider_icon(self.kind.id(), 16.)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .line_height(px(16.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(format!("{} usage", self.kind.label())),
                            )
                            .child(
                                div()
                                    .mt(px(2.))
                                    .text_size(px(10.))
                                    .line_height(px(16.))
                                    .text_color(content(0.4))
                                    .child(updated),
                            )
                            .when(
                                vorkspace_core::accounts::supports_accounts(self.kind),
                                |d| d.child(self.account_line(cx)),
                            ),
                    )
                    .when(state.fetching, |d| {
                        d.child(
                            div()
                                .mt(px(2.))
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_size(px(10.))
                                .text_color(content(0.4))
                                .child(icon("refresh", 10.).text_color(content(0.4)))
                                .child("Updating"),
                        )
                    }),
            )
            .when(
                state.error.is_some() && (has_windows || self.has_banked_resets()),
                |d| {
                    d.child(
                        div()
                            .mb_2()
                            .rounded_lg()
                            .px(px(10.))
                            .py_2()
                            .bg(gpui::rgba(0xfbbf241a))
                            .text_size(px(10.))
                            .line_height(px(16.))
                            .text_color(rgb(0xfcd34d))
                            .child("Couldn’t refresh. Showing the last available snapshot."),
                    )
                },
            )
            .when(!has_windows && !self.has_banked_resets(), |d| {
                let loading = state.fetching || (state.snapshot.is_none() && state.error.is_none());
                d.child(
                    div()
                        .rounded_lg()
                        .bg(content(0.04))
                        .border_1()
                        .border_color(content(0.06))
                        .px_3()
                        .py_4()
                        .flex()
                        .flex_col()
                        .items_center()
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(content(0.65))
                                .child(
                                    if let Some(name) = external_billing(state.snapshot.as_ref()) {
                                        name
                                    } else if loading {
                                        "Loading usage…"
                                    } else if state.error.as_ref().is_some_and(|e| e.unavailable) {
                                        "Not connected"
                                    } else {
                                        "Usage unavailable"
                                    },
                                ),
                        )
                        .children(state.error.as_ref().map(|error| {
                            div()
                                .mt_1()
                                .max_w(px(240.))
                                .text_center()
                                .text_size(px(10.))
                                .line_height(px(16.))
                                .text_color(content(0.4))
                                .child(error.message.clone())
                        })),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .children(windows.map(|w| self.window_card(w, now))),
            )
            .children(
                state
                    .snapshot
                    .as_ref()
                    .and_then(|s| s.extra_usage_balance)
                    .map(|balance| {
                        div()
                            .mt(px(6.))
                            .rounded_lg()
                            .bg(content(0.045))
                            .border_1()
                            .border_color(content(0.06))
                            .px_3()
                            .py(px(10.))
                            .flex()
                            .justify_between()
                            .items_baseline()
                            .gap_3()
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(content(0.65))
                                    .child("Extra usage balance"),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(format!("${balance:.2}")),
                            )
                    }),
            )
            .children(self.suggestion(cx))
            .children(self.banked_resets(cx))
            .into_any()
    }
    /// The active account under the title: "Default account Pro · email ›".
    fn account_line(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let kind = self.kind;
        let id = self.key.profile.id.clone();
        let center = crate::accounts::get(cx);
        let label = center
            .as_ref()
            .and_then(|center| {
                center
                    .read(cx)
                    .book
                    .list(kind)
                    .into_iter()
                    .find(|a| a.id == id)
            })
            .map(|a| a.label)
            .unwrap_or_else(|| "Removed account".into());
        let subtitle = center
            .and_then(|center| center.update(cx, |center, cx| center.subtitle(kind, &id, cx)));
        div()
            .id("usage-switch-account")
            .role(Role::Button)
            .aria_label(format!("Switch {} account", kind.label()))
            .mt_1()
            .ml(px(-4.))
            .max_w_full()
            .px_1()
            .py(px(2.))
            .rounded_sm()
            .flex()
            .items_center()
            .gap_1()
            .text_size(px(10.))
            .text_color(content(0.55))
            .cursor_pointer()
            .hover(|s| s.bg(content(0.1)))
            .child(
                div()
                    .max_w(gpui::relative(0.6))
                    .flex_shrink_0()
                    .truncate()
                    .child(label),
            )
            .children(subtitle.map(|d| d.text_color(content(0.35))))
            .child(icon("chevron-right", 10.).text_color(content(0.55)))
            .on_click(cx.listener(|this, _, window, cx| this.open_accounts(window, cx)))
            .into_any_element()
    }
    fn open_accounts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_reset = None;
        self.show_accounts = true;
        if let Some(center) = crate::accounts::get(cx) {
            center.update(cx, |center, cx| {
                center.open_picker(
                    self.kind,
                    &self.key.profile.id,
                    &self.project,
                    self.session,
                    window,
                    cx,
                )
            });
        }
        cx.notify();
    }
    fn window_card(&self, w: &usage::Window, now: i64) -> gpui::Div {
        let used = w.used_percent.clamp(0., 100.);
        let shown = self.display_percent(used);
        let label = match w.minutes {
            1_440 => "Daily limit",
            10_080 => "Weekly limit",
            43_200 => "Monthly limit",
            _ => "5-hour limit",
        };
        div()
            .rounded_lg()
            .bg(content(0.045))
            .border_1()
            .border_color(content(0.06))
            .px_3()
            .py(px(10.))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_baseline()
                    .gap_3()
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(content(0.65))
                            .child(label),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .font_weight(FontWeight::MEDIUM)
                            .child(format!(
                                "{shown:.0}% {}",
                                if self.show_remaining {
                                    "remaining"
                                } else {
                                    "used"
                                }
                            )),
                    ),
            )
            .child(
                div()
                    .mt_2()
                    .h(px(6.))
                    .w_full()
                    .rounded_full()
                    .bg(content(0.1))
                    .overflow_hidden()
                    .child(
                        div()
                            .h_full()
                            .w(gpui::relative(shown as f32 / 100.))
                            .rounded_full()
                            .bg(usage_color(used)),
                    ),
            )
            .child(
                div()
                    .mt(px(6.))
                    .flex()
                    .justify_between()
                    .gap_3()
                    .text_size(px(10.))
                    .line_height(px(16.))
                    .text_color(content(0.4))
                    .child(format!(
                        "{:.0}% {}",
                        100. - shown,
                        if self.show_remaining {
                            "used"
                        } else {
                            "remaining"
                        }
                    ))
                    .child(div().truncate().child(w.reset_label(now))),
            )
    }
    fn suggestion(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let center = crate::accounts::get(cx)?;
        let account = center
            .read(cx)
            .suggestion(self.kind, &self.key.profile.id)?;
        let kind = self.kind;
        let project = self.project.clone();
        let session = self.session.filter(|_| self.kind == self.context_key.kind);
        let (status, exhausted) = {
            let center = center.read(cx);
            (
                center.status(kind, &account.id),
                center.status(kind, &self.key.profile.id).tone == crate::accounts::Tone::Exhausted,
            )
        };
        Some(
            div()
                .mt_2()
                .rounded_lg()
                .bg(content(0.045))
                .border_1()
                .border_color(content(0.06))
                .px_3()
                .py(px(10.))
                .flex()
                .items_center()
                .gap(px(10.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(10.))
                                .line_height(px(16.))
                                .text_color(content(0.45))
                                .child(format!(
                                    "{} · switch to",
                                    if exhausted {
                                        "Out of usage"
                                    } else {
                                        "Running low"
                                    }
                                )),
                        )
                        .child(
                            div()
                                .mt(px(2.))
                                .flex()
                                .items_center()
                                .gap_2()
                                .min_w_0()
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(account.label.clone()),
                                )
                                .child(
                                    crate::accounts::ui::status_label(&status).text_size(px(10.)),
                                ),
                        ),
                )
                .child(
                    crate::settings::widgets::primary_button(
                        "switch-suggested-account",
                        "Switch account",
                        false,
                    )
                    .h_7()
                    .flex_shrink_0()
                    .px(px(10.))
                    .rounded_md()
                    .text_size(px(11.))
                    .child("Switch")
                    .on_click(cx.listener(move |_, _, _, cx| {
                        center.update(cx, |center, cx| {
                            center.choose(kind, account.id.clone(), project.clone(), session, cx)
                        });
                    })),
                )
                .into_any_element(),
        )
    }
}
// The rail shows remaining capacity consistently, independent of the expanded
// used/remaining preference. Unknown usage has no ring, rather than a false zero.
fn rail_window(snapshot: Option<&usage::Snapshot>) -> Option<&usage::Window> {
    let windows = &snapshot?.windows;
    windows
        .iter()
        .find(|w| w.minutes == 10_080 && w.used_percent.is_finite())
        .or_else(|| {
            windows
                .iter()
                .find(|w| w.minutes == 300 && w.used_percent.is_finite())
        })
        .or_else(|| windows.iter().find(|w| w.used_percent.is_finite()))
}

fn summary_lines(snapshot: Option<&usage::Snapshot>, remaining: bool, now: i64) -> Vec<String> {
    let Some(snapshot) = snapshot else {
        return Vec::new();
    };
    let five = snapshot
        .windows
        .iter()
        .find(|w| w.minutes == 300 && w.used_percent.is_finite());
    let weekly = snapshot
        .windows
        .iter()
        .find(|w| w.minutes == 10_080 && w.used_percent.is_finite());
    let format_window = |w: &usage::Window, prefix: Option<&str>, remaining: bool| {
        let percent = if remaining {
            100. - w.used_percent
        } else {
            w.used_percent
        }
        .clamp(0., 100.);
        let mut text = format!(
            "{}{percent:.0}% {}",
            prefix.map(|p| format!("{p}: ")).unwrap_or_default(),
            if remaining { "left" } else { "used" }
        );
        if w.resets_at.is_some() {
            text.push_str(&format!(" · {}", w.countdown(now)));
        }
        text
    };
    if let (Some(five), Some(weekly)) = (five, weekly) {
        vec![
            format_window(five, Some("5h"), true),
            format_window(weekly, Some("W"), true),
        ]
    } else {
        snapshot
            .windows
            .iter()
            .filter(|w| w.used_percent.is_finite())
            .max_by(|a, b| a.used_percent.total_cmp(&b.used_percent))
            .map(|w| vec![format_window(w, None, remaining)])
            .unwrap_or_default()
    }
}

fn usage_ring(kind: Kind, window: &usage::Window) -> impl IntoElement {
    let ratio = ((100. - window.used_percent).clamp(0., 100.) / 100.) as f32;
    let color: gpui::Hsla = match kind {
        Kind::Codex => rgb(0x7a9dff).into(),
        Kind::Claude => rgb(0xd97757).into(),
        _ => content(0.7),
    };
    gpui::canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let center = bounds.center();
            for (amount, alpha) in [(1., 0.22), (ratio, 1.)] {
                if amount <= 0. {
                    continue;
                }
                let mut path = gpui::PathBuilder::stroke(px(2.));
                let steps = (amount * 100.).ceil() as usize;
                for step in 0..=steps {
                    let angle = step as f32 / steps as f32 * amount * std::f32::consts::TAU
                        - std::f32::consts::FRAC_PI_2;
                    let point = center + gpui::point(px(12. * angle.cos()), px(12. * angle.sin()));
                    if step == 0 {
                        path.move_to(point);
                    } else {
                        path.line_to(point);
                    }
                }
                if amount == 1. {
                    path.close();
                }
                let mut stroke = color;
                stroke.a *= alpha;
                if let Ok(path) = path.build() {
                    window.paint_path(path, stroke);
                }
                if alpha == 1. && amount < 1. {
                    for angle in [
                        -std::f32::consts::FRAC_PI_2,
                        amount * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2,
                    ] {
                        let origin = center
                            + gpui::point(px(12. * angle.cos() - 1.), px(12. * angle.sin() - 1.));
                        window.paint_quad(
                            gpui::fill(
                                gpui::Bounds::new(origin, gpui::size(px(2.), px(2.))),
                                stroke,
                            )
                            .corner_radii(px(1.)),
                        );
                    }
                }
            }
        },
    )
    .absolute()
    .inset_0()
    .size_full()
}

use crate::accounts::ui::bar_color as usage_color;
impl Render for UsageFooter {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let subscriptions = self.subscriptions(cx);
        self.rows
            .retain(|key, _| subscriptions.iter().any(|row| &row.0 == key));
        let now = usage::now_ms();
        let mut rows = Vec::new();
        for (key, name, state) in subscriptions {
            let (anchor, trigger, _) = self
                .rows
                .entry(key.clone())
                .or_insert_with(|| (AnchorSlot::default(), cx.focus_handle(), cx.focus_handle()))
                .clone();
            let detail = state
                .snapshot
                .as_ref()
                .map(|snapshot| {
                    let windows = snapshot
                        .windows
                        .iter()
                        .filter(|window| window.used_percent.is_finite())
                        .map(|window| {
                            let mut text = format!(
                                "{} {}",
                                window.label(),
                                self.percent_label(window.used_percent)
                            );
                            if window.resets_at.is_some() {
                                text.push_str(&format!(" · {}", window.reset_label(now)));
                            } else {
                                text.push_str(" · reset time unavailable");
                            }
                            text
                        })
                        .collect::<Vec<_>>();
                    if windows.is_empty() {
                        external_billing(Some(snapshot))
                            .unwrap_or("Usage unavailable")
                            .into()
                    } else {
                        windows.join("  ·  ")
                    }
                })
                .unwrap_or_else(|| {
                    if state.fetching || state.error.is_none() {
                        "Checking…".into()
                    } else if state.error.as_ref().is_some_and(|error| error.unavailable) {
                        "Not connected".into()
                    } else {
                        "Usage unavailable".into()
                    }
                });
            let mut summary = summary_lines(state.snapshot.as_ref(), self.show_remaining, now);
            if summary.is_empty() {
                summary.push(detail.clone());
            }
            let ring = rail_window(state.snapshot.as_ref());
            let picked = key.clone();
            let row_id = format!(
                "subscription-{}-{}-{}",
                key.kind.id(),
                key.profile.id,
                key.billing
            );
            rows.push(
                div()
                    .id(row_id.clone())
                    .debug_selector(move || row_id.clone())
                    .flex_shrink_0()
                    .min_w_0()
                    .w_full()
                    .flex()
                    .items_center()
                    .when(self.compact, |d| d.justify_center())
                    .gap_1()
                    .child(
                        div()
                            .id(gpui::SharedString::from(format!(
                                "subscription-open-{}-{}-{}",
                                key.kind.id(),
                                key.profile.id,
                                key.billing
                            )))
                            .role(Role::Button)
                            .aria_label(format!("{name} usage details"))
                            .aria_description(if self.compact {
                                match ring {
                                    Some(w) => {
                                        format!("{} remaining shown in ring. {detail}", w.label())
                                    }
                                    None => detail.clone(),
                                }
                            } else if summary.len() == 2 {
                                format!("{}. {detail}", summary.join(". "))
                            } else {
                                detail.clone()
                            })
                            .track_focus(&trigger)
                            .tab_index(0)
                            .key_context("UsageChip")
                            .relative()
                            .min_w_0()
                            .rounded_md()
                            .map(|d| {
                                if self.compact {
                                    d.size(px(32.)).flex_shrink_0().justify_center()
                                } else {
                                    d.flex_1().px_2().py_1p5()
                                }
                            })
                            .flex()
                            .items_center()
                            .gap_2()
                            .hover(|s| s.bg(content(0.08)).text_color(content(1.)))
                            .child(anchor.probe())
                            .when(self.compact, |d| {
                                d.children(ring.map(|w| usage_ring(key.kind, w)))
                            })
                            .child(provider_icon(key.kind.id(), 14.))
                            .when(!self.compact, |d| {
                                d.child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .line_height(px(16.))
                                                .text_color(content(0.75))
                                                .truncate()
                                                .child(name),
                                        )
                                        .children(summary.into_iter().map(|line| {
                                            div()
                                                .text_size(px(10.))
                                                .line_height(px(15.))
                                                .text_color(content(0.5))
                                                .truncate()
                                                .child(line)
                                        })),
                                )
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.request_refresh(picked.clone(), cx);
                                this.open_subscription(picked.clone(), window, cx)
                            }))
                            .on_action(cx.listener(move |this, _: &ToggleUsage, window, cx| {
                                this.request_refresh(key.clone(), cx);
                                this.open_subscription(key.clone(), window, cx)
                            })),
                    )
                    .into_any_element(),
            );
        }
        let weak = cx.weak_entity();
        div()
            .id("usage-footer")
            .debug_selector(|| "subscriptions".into())
            .role(Role::Group)
            .aria_label("Subscriptions")
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .text_color(content(0.55))
            .child(
                div()
                    .id("subscriptions-list")
                    .when(self.compact, |d| d.flex().flex_col().gap_1())
                    .max_h(px(220.))
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows),
            )
            .when(self.open, |d| {
                d.child(
                    popover("usage-details", Anchor::Slot(self.anchor.clone()))
                        .side(Side::Top)
                        // Flush with the row it opens from, and with its corners.
                        .align(crate::popover::Align::Start)
                        .radius(6.)
                        .gap(7.)
                        .width(if self.show_accounts { 340. } else { 300. })
                        .max_height(460.)
                        .padding(if !self.show_accounts && self.login_view(cx) {
                            0.
                        } else {
                            10.
                        })
                        .on_dismiss(move |window, cx| {
                            let _ = weak.update(cx, |this, cx| this.dismiss(window, cx));
                        })
                        .child(self.details(cx)),
                )
            })
    }
}

fn external_billing(snapshot: Option<&usage::Snapshot>) -> Option<&'static str> {
    match snapshot.and_then(|s| s.external_billing.as_deref()) {
        Some("AWS Bedrock") => Some("AWS Bedrock"),
        Some("GitHub Copilot") => Some("GitHub Copilot"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sidebar_usage_windows_resets_and_rail_priority() {
        use super::*;
        let window = |minutes, used_percent, resets_at| usage::Window {
            minutes,
            used_percent,
            resets_at,
        };
        let mut snapshot = usage::Snapshot {
            windows: vec![
                window(10_080, 31., Some(172_800_000)),
                window(300, 0., None),
            ],
            ..Default::default()
        };
        assert_eq!(
            summary_lines(Some(&snapshot), false, 0),
            ["5h: 100% left", "W: 69% left · 2d"]
        );
        assert_eq!(rail_window(Some(&snapshot)).unwrap().minutes, 10_080);
        snapshot.windows[1].resets_at = Some(3_600_000);
        assert_eq!(
            summary_lines(Some(&snapshot), true, 0),
            ["5h: 100% left · 1h", "W: 69% left · 2d"]
        );
        snapshot.windows.remove(1);
        assert_eq!(summary_lines(Some(&snapshot), true, 0), ["69% left · 2d"]);
        assert_eq!(rail_window(Some(&snapshot)).unwrap().minutes, 10_080);
        snapshot.windows = vec![window(300, 0., Some(3_600_000))];
        assert_eq!(summary_lines(Some(&snapshot), false, 0), ["0% used · 1h"]);
        assert_eq!(rail_window(Some(&snapshot)).unwrap().minutes, 300);
        snapshot.windows = vec![window(1_440, 75., None)];
        assert_eq!(summary_lines(Some(&snapshot), true, 0), ["25% left"]);
        assert_eq!(rail_window(Some(&snapshot)).unwrap().minutes, 1_440);
        snapshot.windows[0].used_percent = f64::NAN;
        assert!(summary_lines(Some(&snapshot), true, 0).is_empty());
        assert!(rail_window(Some(&snapshot)).is_none());
        assert!(rail_window(None).is_none());
        snapshot.windows.clear();
        assert!(rail_window(Some(&snapshot)).is_none());
    }

    use super::*;
    #[gpui::test]
    fn subscription_pointer_and_keyboard_activation_refresh_the_selected_account(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(bind);
        let directory = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(std::sync::Mutex::new(
            vorkspace_core::store::Store::open(&directory.path().join("native.sqlite")).unwrap(),
        ));
        let keys = cx.update(|cx| {
            crate::accounts::install(store, cx);
            crate::accounts::get(cx).unwrap().update(cx, |center, cx| {
                let keys = [Kind::Codex, Kind::Claude]
                    .map(|kind| Key::new(kind, center.profile("default"), None));
                for key in &keys {
                    center
                        .usage
                        .entry(key.clone())
                        .or_default()
                        .complete(Ok(Snapshot {
                            windows: vec![usage::Window {
                                used_percent: 10.,
                                minutes: 300,
                                resets_at: None,
                            }],
                            updated_at: usage::now_ms(),
                            ..Default::default()
                        }));
                }
                cx.notify();
                keys
            })
        });
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| UsageFooter::new(Kind::Codex, cx))
            })
            .unwrap()
        });
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let id = |key: &Key| -> &'static str {
            format!(
                "subscription-{}-{}-{}",
                key.kind.id(),
                key.profile.id,
                key.billing
            )
            .leak()
        };
        let a = visual.debug_bounds(id(&keys[0])).unwrap();
        let b = visual.debug_bounds(id(&keys[1])).unwrap();
        assert!(b.origin.y > a.origin.y, "rows stack");
        assert!(a.size.width > px(100.), "rows are detailed, not rings");
        assert!(visual.debug_bounds("subscription-refresh").is_none());
        visual.simulate_click(b.center(), gpui::Modifiers::none());
        visual.run_until_parked();
        let requested = window
            .read_with(&visual, |footer, _| footer.refresh_requests.clone())
            .unwrap();
        assert_eq!(requested, vec![keys[1].clone()]);

        for compact in [false, true] {
            for (keystroke, key) in [("enter", &keys[0]), ("space", &keys[1])] {
                window
                    .update(&mut visual, |footer, window, cx| {
                        footer.dismiss(window, cx);
                        footer.set_compact(compact, cx);
                        footer.refresh_requests.clear();
                        window.focus(&footer.rows[key].1, cx);
                    })
                    .unwrap();
                visual.run_until_parked();
                visual.simulate_keystrokes(keystroke);
                visual.run_until_parked();
                window
                    .update(&mut visual, |footer, _, _| {
                        assert_eq!(footer.refresh_requests, vec![key.clone()]);
                        assert_eq!(footer.open_subscription_key(), Some(key));
                    })
                    .unwrap();
                visual.simulate_keystrokes("escape");
                visual.run_until_parked();
                window
                    .update(&mut visual, |footer, window, _| {
                        assert!(!footer.open);
                        assert!(footer.rows[key].1.is_focused(window));
                        assert_eq!(footer.refresh_requests, vec![key.clone()]);
                    })
                    .unwrap();
            }
        }
    }

    #[gpui::test]
    fn subscriptions_keep_account_usage_separate_from_the_active_conversation(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(bind);
        let directory = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(std::sync::Mutex::new(
            vorkspace_core::store::Store::open(&directory.path().join("native.sqlite")).unwrap(),
        ));
        let work = cx.update(|cx| {
            crate::accounts::install(store, cx);
            crate::accounts::get(cx).unwrap().update(cx, |center, cx| {
                let work = center.book.add(Kind::Codex, "Work").unwrap();
                for (kind, id, percent) in [
                    (Kind::Codex, "default", 20.),
                    (Kind::Claude, "default", 40.),
                    (Kind::Codex, work.id.as_str(), 90.),
                ] {
                    let key = Key::new(kind, center.profile(id), None);
                    center.usage.entry(key).or_default().complete(Ok(Snapshot {
                        windows: vec![usage::Window {
                            used_percent: percent,
                            minutes: 300,
                            resets_at: None,
                        }],
                        updated_at: usage::now_ms(),
                        ..Default::default()
                    }));
                }
                cx.notify();
                Key::new(Kind::Codex, center.profile(&work.id), None)
            })
        });
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| UsageFooter::new(Kind::Codex, cx))
            })
            .unwrap()
        });
        cx.run_until_parked();
        window
            .update(cx, |usage, window, cx| {
                let rows = usage.subscriptions(cx);
                assert_eq!(rows.len(), 3);
                assert!(
                    rows.iter()
                        .any(|row| row.0 == work && row.1 == "Codex · Work")
                );
                usage.open_subscription(work.clone(), window, cx);
                assert_eq!(
                    usage.cache[index(Kind::Codex)]
                        .snapshot
                        .as_ref()
                        .unwrap()
                        .windows[0]
                        .used_percent,
                    90.
                );
                usage.context(
                    Kind::Codex,
                    Default::default(),
                    None,
                    "/work".into(),
                    Some(7),
                    cx,
                );
                assert!(
                    usage.open,
                    "unchanged chat context must not close a different account's details"
                );
                assert_eq!(usage.key, work);
            })
            .unwrap();
        cx.update(|cx| {
            crate::accounts::get(cx).unwrap().update(cx, |center, cx| {
                let key = Key::new(Kind::Codex, Default::default(), None);
                center
                    .usage
                    .get_mut(&key)
                    .unwrap()
                    .snapshot
                    .as_mut()
                    .unwrap()
                    .windows[0]
                    .used_percent = 5.;
                cx.notify();
            });
        });
        cx.run_until_parked();
        window
            .update(cx, |usage, _, cx| {
                assert_eq!(
                    usage.cache[index(Kind::Codex)]
                        .snapshot
                        .as_ref()
                        .unwrap()
                        .windows[0]
                        .used_percent,
                    90.
                );
                usage.set_remaining(true, cx);
                assert_eq!(usage.percent_label(90.), "10% left");
                usage.hide(cx);
                assert!(!usage.open);
                crate::accounts::get(cx).unwrap().update(cx, |center, cx| {
                    center
                        .book
                        .accounts
                        .retain(|account| account.id != work.profile.id);
                    cx.notify();
                });
                assert_eq!(
                    usage.subscriptions(cx).len(),
                    2,
                    "removed accounts cannot survive as stale usage rows"
                );
                let bedrock = Key::new(Kind::Claude, Default::default(), None);
                crate::accounts::get(cx).unwrap().update(cx, |center, cx| {
                    center
                        .usage
                        .get_mut(&bedrock)
                        .unwrap()
                        .complete(Ok(Snapshot {
                            external_billing: Some("AWS Bedrock".into()),
                            updated_at: usage::now_ms(),
                            ..Default::default()
                        }));
                    cx.notify();
                });
                assert!(
                    usage.subscriptions(cx).iter().any(|row| row.0 == bedrock),
                    "Bedrock must remain visible without subscription limit windows"
                );
                usage.select_subscription(bedrock, cx);
                let snapshot = usage.cache[index(Kind::Claude)].snapshot.as_ref();
                assert_eq!(external_billing(snapshot), Some("AWS Bedrock"));
                assert!(
                    !usage.login_view(cx),
                    "Bedrock must not prompt for Claude OAuth"
                );
                assert!(snapshot.unwrap().windows.is_empty());
            })
            .unwrap();
        // Without a cost monitoring link the row keeps opening its details.
        window
            .update(cx, |usage, w, cx| {
                let bedrock = Key::new(Kind::Claude, Default::default(), None);
                usage.open_subscription(bedrock, w, cx);
                assert!(usage.open);
                usage.hide(cx);
            })
            .unwrap();
        assert_eq!(cx.opened_url(), None);
        cx.update(|cx| {
            cx.set_global(crate::settings::Preferences {
                bedrock_cost_url: " https://console.aws.amazon.com/costmanagement/ ".into(),
                ..Default::default()
            })
        });
        window
            .update(cx, |usage, w, cx| {
                let bedrock = Key::new(Kind::Claude, Default::default(), None);
                usage.open_subscription(bedrock, w, cx);
                assert!(!usage.open, "the link replaces the details popover");
            })
            .unwrap();
        assert_eq!(
            cx.opened_url().as_deref(),
            Some("https://console.aws.amazon.com/costmanagement/")
        );
    }
    #[test]
    fn cost_link_accepts_only_plain_web_links() {
        let link = |url: &str| {
            crate::settings::Preferences {
                bedrock_cost_url: url.into(),
                ..Default::default()
            }
            .cost_link()
            .map(str::to_owned)
        };
        assert_eq!(
            link(" http://a.example/x ").as_deref(),
            Some("http://a.example/x")
        );
        for bad in [
            "",
            "console.aws.amazon.com",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "https://",
            "https:///x",
            "https://a b.example",
            "ftp://a.example",
        ] {
            assert_eq!(link(bad), None, "{bad}");
        }
    }
    #[gpui::test]
    fn details_open_with_keyboard_escape_restores_focus_and_provider_caches_stay_separate(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(bind);
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| UsageFooter::new(Kind::Codex, cx))
            })
            .unwrap()
        });
        window
            .update(cx, |footer, w, cx| {
                footer.cache[0].complete(Ok(Snapshot {
                    windows: vec![usage::Window {
                        used_percent: 28.,
                        minutes: 300,
                        resets_at: None,
                    }],
                    updated_at: usage::now_ms(),
                    ..Default::default()
                }));
                w.focus(&footer.trigger, cx);
            })
            .unwrap();
        cx.refresh().unwrap();
        cx.simulate_keystrokes(window.into(), "enter");
        cx.run_until_parked();
        window
            .update(cx, |footer, w, cx| {
                assert!(footer.open);
                assert!(footer.details.is_focused(w));
                cx.notify();
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "escape");
        cx.run_until_parked();
        window
            .update(cx, |footer, w, cx| {
                assert!(!footer.open);
                assert!(footer.trigger.is_focused(w));
                footer.provider(Kind::Claude, cx);
                assert!(footer.cache[index(footer.kind)].snapshot.is_none());
                // A late Codex response belongs to Codex, while Claude stays in view.
                footer.cache[0].complete(Ok(Snapshot {
                    windows: vec![usage::Window {
                        used_percent: 42.,
                        minutes: 300,
                        resets_at: None,
                    }],
                    updated_at: usage::now_ms(),
                    ..Default::default()
                }));
                assert!(footer.cache[index(footer.kind)].snapshot.is_none());
                footer.provider(Kind::Codex, cx);
                assert_eq!(
                    footer.cache[0].snapshot.as_ref().unwrap().windows[0].used_percent,
                    42.
                );
            })
            .unwrap();
    }
    #[test]
    fn failed_refresh_keeps_snapshot_but_sign_out_clears_it() {
        let mut state = CachedUsage {
            snapshot: Some(Snapshot::default()),
            fetching: true,
            ..Default::default()
        };
        state.complete(Err(FetchError {
            message: "Offline".into(),
            unavailable: false,
        }));
        assert!(state.snapshot.is_some());
        assert!(!state.fetching);
        state.complete(Err(FetchError {
            message: "Signed out".into(),
            unavailable: true,
        }));
        assert!(state.snapshot.is_none());
        state.complete(Ok(Snapshot::default()));
        assert!(state.error.is_none());
    }
}
