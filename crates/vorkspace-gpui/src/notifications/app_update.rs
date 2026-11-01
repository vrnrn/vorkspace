//! Installed-update card and bundled What's new content. The app updater itself
//! is a separate backend concern; observing a new version records its notice.
use crate::markdown::{self, MarkdownRow, Tone};
use gpui::{Context, EventEmitter, FontWeight, Render, Role, Window, div, img, prelude::*, px};
use vorkspace_core::store::Store;
use vorkspace_model::markdown_model;
use vorkspace_ui::clock::Instant;
use vorkspace_ui::theme::*;
const MARKER: &str = "vorkspace.installedUpdate";
const VERSION: &str = "vorkspace.lastSeenAppVersion";

/// Consume before parsing, matching React's one-launch marker semantics. The
/// settings abstraction has no delete operation, so an empty value is cleared.
pub(crate) fn consume_installed_update(store: &Store) -> Result<Option<String>, String> {
    observe_version(store, env!("CARGO_PKG_VERSION"))
}
fn observe_version(store: &Store, version: &str) -> Result<Option<String>, String> {
    let previous = store.setting(VERSION)?;
    // DesktopPreferences already canonicalizes historical app-owned prefixes.
    // A cleared native marker takes precedence over immutable WebKit storage.
    let raw = store.setting(MARKER)?.or_else(|| {
        store
            .desktop_preferences()
            .and_then(|p| p.get(MARKER))
            .map(str::to_owned)
    });
    let installed = raw
        .as_ref()
        .filter(|value| !value.is_empty())
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok());
    if raw.as_ref().is_some_and(|value| !value.is_empty()) {
        store.save_setting(MARKER, "")?;
    }
    let explicit = installed
        .as_ref()
        .and_then(|v| v["version"].as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .is_some_and(|installed| installed == version);
    if previous.as_deref() != Some(version) {
        store.save_setting(VERSION, version)?;
    }
    Ok(
        (previous.as_deref() != Some(version) && (previous.is_some() || explicit))
            .then(|| version.to_owned()),
    )
}
pub(crate) enum AppUpdateEvent {
    Open(String),
    Dismiss,
}
pub(crate) struct InstalledUpdateNotice {
    version: Option<String>,
}
impl EventEmitter<AppUpdateEvent> for InstalledUpdateNotice {}
impl InstalledUpdateNotice {
    pub(crate) fn new(version: Option<String>) -> Self {
        Self { version }
    }
    pub(crate) fn set_version(&mut self, version: Option<String>, cx: &mut Context<Self>) {
        if self.version != version {
            self.version = version;
            cx.notify();
        }
    }
    pub(crate) fn is_visible(&self) -> bool {
        self.version.is_some()
    }
}
impl Render for InstalledUpdateNotice {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(version) = self.version.clone() else {
            return div().into_any_element();
        };
        let label = format!("Updated to {version}");
        div()
            .id("installed-update-notice")
            .role(Role::Status)
            .relative()
            .overflow_hidden()
            .rounded(px(8.))
            .bg(content(0.12))
            .child(
                div()
                    .id("open-installed-update")
                    .role(Role::Button)
                    .aria_label(label.clone())
                    .cursor_pointer()
                    .flex()
                    .w_full()
                    .items_start()
                    .gap(px(8.))
                    .rounded(px(8.))
                    .p(px(8.))
                    .pr(px(32.))
                    .hover(|s| {
                        let mut accent = accent_color();
                        accent.a = 0.1;
                        s.bg(accent)
                    })
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(AppUpdateEvent::Open(version.clone()))
                    }))
                    .child(
                        div()
                            .mt(px(2.))
                            .size(px(18.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(img("vorkspace.png").size(px(16.))),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(label),
                            )
                            .child(
                                div()
                                    .mt(px(2.))
                                    .text_size(px(11.))
                                    .text_color(content(0.5))
                                    .child("What's new"),
                            ),
                    ),
            )
            .child(
                div()
                    .id("dismiss-installed-update")
                    .role(Role::Button)
                    .aria_label("Dismiss update notification")
                    .absolute()
                    .right(px(4.))
                    .top(px(4.))
                    .size(px(24.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(content(0.45))
                    .hover(|s| s.bg(content(0.08)).text_color(content(1.)))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(AppUpdateEvent::Dismiss)))
                    .child(icon("x", 14.)),
            )
            .into_any_element()
    }
}
struct ReleaseNotes {
    date: Option<String>,
    markdown: String,
}
fn release_notes(version: &str) -> Option<ReleaseNotes> {
    let version = version.trim();
    if version.is_empty() || version == "Unreleased" {
        return None;
    }
    let source = include_str!(concat!(env!("OUT_DIR"), "/release-notes.md"));
    parse_release_notes(version, source)
}
fn parse_release_notes(version: &str, source: &str) -> Option<ReleaseNotes> {
    let version = version.trim();
    if version.is_empty() || version == "Unreleased" {
        return None;
    }
    let heading = format!("## [{version}]");
    let mut found = false;
    let mut date = None;
    let mut body = Vec::new();
    for line in source.lines() {
        if !found {
            let Some(suffix) = line.strip_prefix(&heading) else {
                continue;
            };
            if suffix.is_empty() {
                found = true;
            } else if let Some(iso) = suffix.strip_prefix(" - ")
                && iso.len() == 10
                && iso.bytes().enumerate().all(|(i, b)| {
                    if i == 4 || i == 7 {
                        b == b'-'
                    } else {
                        b.is_ascii_digit()
                    }
                })
            {
                date = Some(format_date(iso));
                found = true;
            }
        } else if version_heading(line) {
            break;
        } else {
            body.push(line);
        }
    }
    found.then(|| ReleaseNotes {
        date,
        markdown: body.join("\n").trim().to_owned(),
    })
}
fn version_heading(line: &str) -> bool {
    let Some(after) = line.strip_prefix("## [") else {
        return false;
    };
    let Some((version, suffix)) = after.split_once(']') else {
        return false;
    };
    !version.is_empty()
        && !version.contains(['\r', '\n'])
        && (suffix.is_empty()
            || suffix.strip_prefix(" - ").is_some_and(|date| {
                date.len() == 10
                    && date.bytes().enumerate().all(|(i, b)| {
                        if i == 4 || i == 7 {
                            b == b'-'
                        } else {
                            b.is_ascii_digit()
                        }
                    })
            }))
}
fn format_date(iso: &str) -> String {
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month = iso[5..7]
        .parse::<usize>()
        .ok()
        .and_then(|n| n.checked_sub(1))
        .and_then(|n| months.get(n));
    match month {
        Some(month) => format!(
            "{} {month} {}",
            iso[8..10].parse::<u32>().unwrap_or(0),
            &iso[..4]
        ),
        None => iso.into(),
    }
}
pub(crate) enum WhatsNewEvent {
    Close,
}
pub(crate) struct WhatsNew {
    version: String,
    date: Option<String>,
    rows: Vec<MarkdownRow>,
    loading: bool,
    error: Option<String>,
    load: fn(&str) -> Result<Option<String>, String>,
}
impl EventEmitter<WhatsNewEvent> for WhatsNew {}
impl WhatsNew {
    pub(crate) fn new(version: String, cx: &mut Context<Self>) -> Self {
        Self::with_loader(version, vorkspace_core::release_notes::load, cx)
    }
    fn with_loader(
        version: String,
        load: fn(&str) -> Result<Option<String>, String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let notes = release_notes(&version);
        let date = notes.as_ref().and_then(|n| n.date.clone());
        let rows = notes
            .map(|notes| {
                markdown_model::parse_full(&notes.markdown)
                    .blocks
                    .iter()
                    .flat_map(|b| markdown::rows(&b.block))
                    .collect()
            })
            .unwrap_or_default();
        let mut view = Self {
            version,
            date,
            rows,
            loading: false,
            error: None,
            load,
        };
        if view.rows.is_empty() {
            view.load_published(cx);
        }
        view
    }
    fn load_published(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        self.error = None;
        let version = self.version.clone();
        let load = self.load;
        let work = cx
            .background_executor()
            .spawn(async move { load(&version) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(Some(source)) => {
                        if let Some(notes) = parse_release_notes(&this.version, &source) {
                            this.date = notes.date;
                            this.rows = markdown_model::parse_full(&notes.markdown)
                                .blocks
                                .iter()
                                .flat_map(|block| markdown::rows(&block.block))
                                .collect();
                        }
                    }
                    Ok(None) => {}
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
impl Render for WhatsNew {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let description = format!(
            "vorkspace {}{}",
            self.version,
            self.date
                .as_ref()
                .map(|d| format!(" · {d}"))
                .unwrap_or_default()
        );
        let mut body = div()
            .id("whats-new-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(20.))
            .py(px(16.));
        if self.rows.is_empty() {
            body = body.child(div().text_size(px(13.)).text_color(content(0.6)).child(
                if self.loading {
                    "Loading release notes…"
                } else if self.error.is_some() {
                    "Could not load release notes."
                } else {
                    "Release notes for this version are not available."
                },
            ));
            if self.error.is_some() {
                body = body.child(
                    div()
                        .id("retry-release-notes")
                        .role(Role::Button)
                        .cursor_pointer()
                        .mt_3()
                        .child("Retry")
                        .on_click(cx.listener(|this, _, _, cx| this.load_published(cx))),
                );
            }
        }
        for (ix, row) in self.rows.iter().enumerate() {
            body = body.child(div().pt(px(if ix == 0 { 0. } else { row.gap })).child(
                markdown::body(
                    row,
                    format!("whats-new-row-{ix}").into(),
                    Tone::Normal,
                    None,
                    Instant::now(),
                ),
            ));
        }
        div()
            .id("whats-new-dialog")
            .role(Role::Dialog)
            .aria_label("What's new")
            .relative()
            .w(px(512.))
            .max_w_full()
            .h((window.viewport_size().height * 0.72).min(px(640.)))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(12.))
            .border_1()
            .border_color(content(0.1))
            .text_color(content(1.))
            .shadow_lg()
            .child(super::surface::backdrop(12.))
            .child(
                div()
                    .relative()
                    .px(px(20.))
                    .py(px(16.))
                    .border_b_1()
                    .border_color(content(0.1))
                    .flex()
                    .items_start()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex_1()
                            .child(
                                div()
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("What's new"),
                            )
                            .child(
                                div()
                                    .mt(px(4.))
                                    .text_size(px(12.))
                                    .text_color(content(0.6))
                                    .child(description),
                            ),
                    )
                    .child(
                        div()
                            .id("close-whats-new")
                            .role(Role::Button)
                            .aria_label("Close")
                            .cursor_pointer()
                            .size(px(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(WhatsNewEvent::Close)))
                            .child(icon("x", 14.)),
                    ),
            )
            .child(body.relative())
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn github_section_headings_stay_inside_the_requested_version() {
        let notes = "## [1.0.0]\n\n## What's Changed\n- Preserve settings.\n\n## Contributors\n- Team\n\n## [0.13.13] - 2026-10-22\nOlder notes";
        let parsed = parse_release_notes("1.0.0", notes).unwrap();
        assert_eq!(
            parsed.markdown,
            "## What's Changed\n- Preserve settings.\n\n## Contributors\n- Team"
        );
        let older = parse_release_notes("0.13.13", notes).unwrap();
        assert_eq!(older.markdown, "Older notes");
        assert_eq!(older.date.as_deref(), Some("22 Oct 2026"));
        assert!(parse_release_notes("1.0.1", notes).is_none());
    }
    #[test]
    fn installed_version_is_observed_once_across_windows_and_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vorkspace.db");
        let store = Store::open_vorkspace(&path).unwrap();
        assert_eq!(observe_version(&store, "1.0.0").unwrap(), None);
        assert_eq!(observe_version(&store, "1.0.0").unwrap(), None);
        assert_eq!(
            observe_version(&store, "1.0.1").unwrap(),
            Some("1.0.1".into())
        );
        assert_eq!(observe_version(&store, "1.0.1").unwrap(), None);
        drop(store);
        let store = Store::open_vorkspace(&path).unwrap();
        assert_eq!(observe_version(&store, "1.0.1").unwrap(), None);
        store
            .save_setting(MARKER, r#"{"version":"1.0.2"}"#)
            .unwrap();
        assert_eq!(
            observe_version(&store, "1.0.2").unwrap(),
            Some("1.0.2".into())
        );
        assert_eq!(store.setting(MARKER).unwrap().as_deref(), Some(""));
        assert_eq!(observe_version(&store, "1.0.2").unwrap(), None);
    }
    #[gpui::test]
    fn missing_bundled_version_loads_complete_published_notes(cx: &mut gpui::TestAppContext) {
        fn load(version: &str) -> Result<Option<String>, String> {
            assert_eq!(version, "9.9.9");
            Ok(Some(
                "## [9.9.9]\n\n## What's Changed\n\n- Preserve settings and drafts.".into(),
            ))
        }
        let view = cx.new(|cx| WhatsNew::with_loader("9.9.9".into(), load, cx));
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(!view.loading);
            assert!(view.error.is_none());
            assert!(!view.rows.is_empty());
        });
    }
}
