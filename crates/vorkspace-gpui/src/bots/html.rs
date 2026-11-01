//! Generated HTML lives in an opaque sandbox with no app bridge or network.
use crate::native_view::invalidate_parent_cursor_rects;
use gpui::{Context, Window, canvas, prelude::*};
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained,
    runtime::ProtocolObject,
};
use objc2_app_kit::{NSView, NSWindowOrderingMode};
use objc2_foundation::{NSObject, NSObjectProtocol};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use objc2_web_kit::{
    WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate, WKNavigationType,
    WKWebView, WKWebViewConfiguration, WKWebsiteDataStore,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{cell::RefCell, rc::Rc, sync::Arc};

define_class!(
    #[unsafe(super(NSView))]
    #[name = "VorkspaceArtifactClip"]
    #[thread_kind = MainThreadOnly]
    #[ivars = std::cell::Cell<bool>]
    struct ArtifactClip;
    unsafe impl NSObjectProtocol for ArtifactClip {}
    impl ArtifactClip {
        #[unsafe(method(isFlipped))]
        fn flipped(&self) -> bool { true }

        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            if !self.ivars().get() {
                None
            } else {
                unsafe { msg_send![super(self), hitTest: point] }
            }
        }
    }
);
define_class!(
    #[unsafe(super(NSObject))]
    #[name = "VorkspaceArtifactNavigation"]
    #[thread_kind=MainThreadOnly]
    #[ivars=gpui::AsyncApp]
    struct NavigationGuard;
    unsafe impl NSObjectProtocol for NavigationGuard {}
    unsafe impl WKNavigationDelegate for NavigationGuard {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        unsafe fn decide(
            &self,
            _: &WKWebView,
            action: &WKNavigationAction,
            reply: &block2::DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            let url = unsafe { action.request().URL() }.and_then(|url| url.absoluteString());
            if unsafe { action.navigationType() } == WKNavigationType::LinkActivated
                && let Some(url) = url.as_ref()
                && (url.to_string().starts_with("https://")
                    || url.to_string().starts_with("http://"))
            {
                let url = url.to_string();
                self.ivars().clone().update(|cx| cx.open_url(&url));
            }
            let allowed = url.is_none_or(|url| {
                let s = url.to_string();
                s.starts_with("about:blank") || s.starts_with("about:srcdoc")
            });
            reply.call((if allowed {
                WKNavigationActionPolicy::Allow
            } else {
                WKNavigationActionPolicy::Cancel
            },));
        }
    }
);
struct Hosted {
    clip: Retained<ArtifactClip>,
    web: Retained<WKWebView>,
    _guard: Retained<NavigationGuard>,
    markup: Arc<str>,
}
struct Markup {
    ink: gpui::Hsla,
    accent: gpui::Hsla,
    html: Arc<str>,
}
pub(super) struct Html {
    body: String,
    interactive: bool,
    markup: Option<Markup>,
    view: Rc<RefCell<Option<Hosted>>>,
}
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
fn sandbox(body: &str) -> String {
    let color = |value: gpui::Hsla| {
        let value: gpui::Rgba = value.into();
        format!(
            "#{:02x}{:02x}{:02x}",
            (value.r * 255.) as u8,
            (value.g * 255.) as u8,
            (value.b * 255.) as u8
        )
    };
    let ink: gpui::Rgba = vorkspace_ui::theme::content(1.).into();
    let foreground = color(vorkspace_ui::theme::content(1.));
    let accent = color(vorkspace_ui::theme::accent_color());
    let light = ink.r * 0.2126 + ink.g * 0.7152 + ink.b * 0.0722 < 0.5;
    let background = if light { "#fafafa" } else { "#141414" };
    let muted = if light { "#737373" } else { "#969696" };
    let surface = if light { "#eeeeee" } else { "#222222" };
    let border = if light { "#d4d4d4" } else { "#383838" };
    let source = format!(
        "<!doctype html><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; script-src 'unsafe-inline' https:; style-src 'unsafe-inline' https:; img-src data: blob: https:; font-src data: https:; media-src data: blob: https:; worker-src blob:; connect-src 'none'; frame-src 'none'; base-uri 'none'; form-action 'none'\"><meta name=viewport content=\"width=device-width,initial-scale=1\"><style>:root{{--background:{background};--foreground:{foreground};--muted:{muted};--surface:{surface};--border:{border};--accent:{accent};--link:{accent};--positive:#10b981;--negative:#f43f5e;--chart-1:{accent};--chart-2:#10b981;--chart-3:#f59e0b;--chart-4:#f43f5e;--chart-5:#a78bfa;--chart-6:#22d3ee;--font-sans:system-ui;--font-bot:ui-monospace,monospace;--radius:10px;color-scheme:{scheme}}}body{{font:14px var(--font-sans);margin:20px;color:var(--foreground);background:var(--background)}}a{{color:var(--link)}}</style>{body}{diagrams}",
        scheme = if light { "light" } else { "dark" },
        diagrams = mermaid_support(body),
    );
    format!(
        "<!doctype html><style>html,body,iframe{{margin:0;border:0;width:100%;height:100%;overflow:hidden;background:{background}}}</style><iframe sandbox=\"allow-scripts\" referrerpolicy=\"no-referrer\" srcdoc=\"{}\"></iframe>",
        escape(&source)
    )
}
/// Pele draws `<pre class="mermaid">` and ```mermaid code blocks in the
/// document's own sandbox, themed by its variables. Only documents that use
/// Mermaid carry the renderer.
fn mermaid_support(body: &str) -> String {
    if !uses_mermaid_class(body) {
        return String::new();
    }
    format!(
        "<style>.pele{{--pele-bg:var(--background);--pele-fg:var(--foreground);--pele-muted:var(--muted);--pele-line:var(--muted);--pele-surface:var(--surface);--pele-surface-alt:var(--border);--pele-border:var(--border);--pele-accent:var(--accent);--pele-font:var(--font-sans)}}</style><script>{}</script><script>for(const el of document.querySelectorAll('pre.mermaid,code.language-mermaid')){{const host=el.tagName==='CODE'&&el.parentElement&&el.parentElement.tagName==='PRE'?el.parentElement:el;try{{pele.mount(host,el.textContent)}}catch(e){{}}}}</script>",
        include_str!("../../assets/pele.global.js")
    )
}
/// Whether any `class` attribute holds a `mermaid` or `language-mermaid`
/// token, in any quote style or order. The mount script's selectors then
/// decide what is drawn.
fn uses_mermaid_class(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    let mut rest = lower.as_str();
    while let Some(at) = rest.find("class") {
        let before = rest[..at].chars().next_back();
        let after = rest[at + "class".len()..].trim_start();
        rest = &rest[at + "class".len()..];
        if !before.is_some_and(char::is_whitespace) {
            continue;
        }
        let Some(value) = after.strip_prefix('=').map(str::trim_start) else {
            continue;
        };
        let value = match value.chars().next() {
            Some(quote @ ('"' | '\'')) => value[1..].split(quote).next().unwrap_or_default(),
            _ => value
                .split(|c: char| c.is_whitespace() || c == '>')
                .next()
                .unwrap_or_default(),
        };
        if value
            .split_whitespace()
            .any(|token| matches!(token, "mermaid" | "language-mermaid"))
        {
            return true;
        }
    }
    false
}
impl Html {
    pub(super) fn new(body: String) -> Self {
        Self {
            body,
            interactive: true,
            markup: None,
            view: Rc::new(RefCell::new(None)),
        }
    }
    fn markup(&mut self) -> Arc<str> {
        let ink = vorkspace_ui::theme::content(1.);
        let accent = vorkspace_ui::theme::accent_color();
        if self
            .markup
            .as_ref()
            .is_none_or(|m| m.ink != ink || m.accent != accent)
        {
            self.markup = Some(Markup {
                ink,
                accent,
                html: sandbox(&self.body).into(),
            });
        }
        self.markup.as_ref().unwrap().html.clone()
    }
    pub(super) fn set_interactive(&mut self, interactive: bool) {
        if self.interactive != interactive {
            self.interactive = interactive;
        }
    }
    pub(super) fn hide(&self) {
        #[cfg(target_os = "macos")]
        if let Some(view) = self.view.borrow().as_ref()
            && !view.clip.isHidden()
        {
            view.clip.setHidden(true);
            invalidate_parent_cursor_rects(&view.clip);
        }
    }
}
impl gpui::Render for Html {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        {
            let view = self.view.clone();
            let html = self.markup();
            let app = cx.to_async();
            let interactive = self.interactive;
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let Ok(handle) = HasWindowHandle::window_handle(window) else {
                        return;
                    };
                    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
                        return;
                    };
                    // GPUI keeps its AppKit content view alive throughout paint.
                    let parent = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
                    let clipped = bounds.intersect(&window.content_mask().bounds);
                    let mut view = view.borrow_mut();
                    if clipped.size.width <= gpui::px(0.) || clipped.size.height <= gpui::px(0.) {
                        if let Some(view) = view.as_ref()
                            && !view.clip.isHidden()
                        {
                            view.clip.setHidden(true);
                            invalidate_parent_cursor_rects(&view.clip);
                        }
                        return;
                    }
                    let scale = f64::from(window.ui_scale());
                    let x = f64::from(clipped.origin.x) * scale;
                    let y = f64::from(clipped.origin.y) * scale;
                    let clip_width = f64::from(clipped.size.width) * scale;
                    let clip_height = f64::from(clipped.size.height) * scale;
                    let width = f64::from(bounds.size.width) * scale;
                    let height = f64::from(bounds.size.height) * scale;
                    let y = if parent.isFlipped() {
                        y
                    } else {
                        parent.bounds().size.height - y - clip_height
                    };
                    let rect =
                        NSRect::new(NSPoint::new(x, y), NSSize::new(clip_width, clip_height));
                    // Keep the document at its full layout size while its
                    // native host follows GPUI's moving content mask.
                    let web_rect = NSRect::new(
                        NSPoint::new(
                            f64::from(bounds.left() - clipped.left()) * scale,
                            f64::from(bounds.top() - clipped.top()) * scale,
                        ),
                        NSSize::new(width, height),
                    );
                    if view.is_none() {
                        let Some(mtm) = MainThreadMarker::new() else {
                            return;
                        };
                        // No message handlers, file base URL,
                        // app credentials or persistent website data are installed.
                        let hosted = unsafe {
                            let clip = ArtifactClip::alloc(mtm)
                                .set_ivars(std::cell::Cell::new(interactive));
                            let clip: Retained<ArtifactClip> =
                                msg_send![super(clip), initWithFrame: rect];
                            clip.setWantsLayer(true);
                            clip.setClipsToBounds(true);
                            let config = WKWebViewConfiguration::new(mtm);
                            vorkspace_macos::webkit::name_process(&config);
                            config
                                .preferences()
                                .setJavaScriptCanOpenWindowsAutomatically(false);
                            config.setWebsiteDataStore(
                                &WKWebsiteDataStore::nonPersistentDataStore(mtm),
                            );
                            let web = WKWebView::initWithFrame_configuration(
                                WKWebView::alloc(mtm),
                                web_rect,
                                &config,
                            );
                            let guard = NavigationGuard::alloc(mtm).set_ivars(app.clone());
                            let guard: Retained<NavigationGuard> = msg_send![super(guard), init];
                            web.setNavigationDelegate(Some(ProtocolObject::from_ref(&*guard)));
                            web.loadHTMLString_baseURL(&NSString::from_str(&html), None);
                            clip.addSubview(&web);
                            Hosted {
                                clip,
                                web,
                                _guard: guard,
                                markup: html.clone(),
                            }
                        };
                        *view = Some(hosted);
                    }
                    if let Some(view) = view.as_mut() {
                        if !Arc::ptr_eq(&view.markup, &html) {
                            unsafe {
                                view.web
                                    .loadHTMLString_baseURL(&NSString::from_str(&html), None);
                            }
                            view.markup = html.clone();
                        }
                        let mut cursor_rects_changed = unsafe { view.clip.superview() }
                            .as_deref()
                            .is_none_or(|superview| !std::ptr::eq(superview, parent));
                        if cursor_rects_changed {
                            parent.addSubview_positioned_relativeTo(
                                &view.clip,
                                NSWindowOrderingMode::Above,
                                None,
                            );
                        }
                        view.clip.ivars().set(interactive);
                        if view.clip.frame() != rect {
                            view.clip.setFrame(rect);
                            cursor_rects_changed = true;
                        }
                        view.web.setFrame(web_rect);
                        // The owned WebKit view is used only on the main thread.
                        unsafe {
                            if view.web.pageZoom() != scale {
                                view.web.setPageZoom(scale);
                            }
                        }
                        if view.clip.isHidden() {
                            view.clip.setHidden(false);
                            cursor_rects_changed = true;
                        }
                        if cursor_rects_changed {
                            invalidate_parent_cursor_rects(&view.clip);
                        }
                    }
                },
            )
            .size_full()
            .into_any_element()
        }
    }
}
impl Drop for Html {
    fn drop(&mut self) {
        if let Some(view) = self.view.borrow_mut().take() {
            view.clip.removeFromSuperview();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generated_markup_cannot_escape_the_opaque_iframe() {
        let html = sandbox("\"><script>window.top.postMessage('x','*')</script>");
        assert!(html.contains("sandbox=\"allow-scripts\""));
        assert!(!html.contains("allow-same-origin"));
        assert!(html.contains("connect-src 'none'"));
        assert!(html.contains("&lt;script&gt;"));
    }
    #[test]
    fn only_mermaid_documents_carry_pele() {
        assert!(!sandbox("<p>No diagrams</p>").contains("pele.mount"));
        let html = sandbox("<pre class=\"mermaid\">flowchart LR\n A --> B</pre>");
        assert!(html.contains("pele.mount"));
        assert!(html.contains("--pele-bg:var(--background)"));
        // Still inside the opaque iframe's escaped source.
        assert!(!html.contains("<script>"));
        // Any quote style or class order.
        for body in [
            "<pre class='mermaid'>flowchart LR\n A --> B</pre>",
            "<pre class=\"diagram mermaid\">flowchart LR\n A --> B</pre>",
            "<PRE CLASS = mermaid>flowchart LR\n A --> B</PRE>",
            "<pre><code class=\"hljs language-mermaid\">flowchart LR</code></pre>",
        ] {
            assert!(sandbox(body).contains("pele.mount"), "{body}");
        }
        // Mentions and lookalike classes carry nothing.
        for body in [
            "<p>Mermaid diagrams are supported.</p>",
            "<pre class=\"mermaid-like\">x</pre>",
            "<pre data-class=\"mermaid\">x</pre>",
        ] {
            assert!(!sandbox(body).contains("pele.mount"), "{body}");
        }
    }
}
