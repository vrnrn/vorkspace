//! Opaque HTML surfaces. Only explicit plugin surfaces receive a scoped bridge.
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
    WKScriptMessage, WKScriptMessageHandler, WKUserContentController, WKWebView,
    WKWebViewConfiguration, WKWebsiteDataStore,
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
pub(crate) type Reply = Rc<dyn Fn(Result<serde_json::Value, String>)>;
pub(crate) type RequestHandler = Rc<dyn Fn(serde_json::Value, Reply)>;
struct Bridge {
    nonce: String,
    handler: RequestHandler,
}
define_class!(
    #[unsafe(super(NSObject))]
    #[name="VorkspacePluginMessageGuard"]
    #[thread_kind=MainThreadOnly]
    #[ivars=Rc<Bridge>]
    struct MessageGuard;
    unsafe impl NSObjectProtocol for MessageGuard {}
    unsafe impl WKScriptMessageHandler for MessageGuard {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        unsafe fn receive(&self,_:&WKUserContentController,message:&WKScriptMessage){
            if !unsafe{message.frameInfo().isMainFrame()}{return}
            let Ok(body)=unsafe{message.body()}.downcast::<NSString>()else{return};
            let body=body.to_string();
            if body.len()>vorkspace_plugin::MAX_FRAME{return}
            let Ok(mut value)=serde_json::from_str::<serde_json::Value>(&body)else{return};
            if value["nonce"].as_str()!=Some(&self.ivars().nonce){return}
            let Some(id)=value["id"].as_u64()else{return};
            let Some(web)=(unsafe{message.webView()})else{return};
            value.as_object_mut().map(|value|value.remove("nonce"));
            let reply:Reply=Rc::new(move|result|{
                let data=match result{
                    Ok(value)=>serde_json::json!({"kind":"vorkspace.reply","id":id,"result":value}),
                    Err(error)=>serde_json::json!({"kind":"vorkspace.reply","id":id,"error":error}),
                };
                let script=format!("document.querySelector('iframe').contentWindow.postMessage({},'*')",data);
                unsafe{web.evaluateJavaScript_completionHandler(&NSString::from_str(&script),None);}
            });
            (self.ivars().handler)(value,reply);
        }
    }
);
struct Hosted {
    clip: Retained<ArtifactClip>,
    web: Retained<WKWebView>,
    _guard: Retained<NavigationGuard>,
    markup: Arc<str>,
    _messages: Option<Retained<MessageGuard>>,
    theme: (gpui::Hsla, gpui::Hsla),
}
struct Markup {
    ink: gpui::Hsla,
    accent: gpui::Hsla,
    html: Arc<str>,
}
pub(crate) struct Html {
    bridge: Option<Rc<Bridge>>,
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

fn plugin_sandbox(body: &str, nonce: &str) -> String {
    let sdk = r#"<script>
(()=>{let next=0;const pending=new Map();
const request=(route,method,params={})=>new Promise((resolve,reject)=>{const id=++next;pending.set(id,{resolve,reject});parent.postMessage({kind:'vorkspace.request',id,route,method,params},'*')});
window.vorkspace={call:(method,params)=>request('plugin',method,params),host:(method,params)=>request('host',method,params)};
addEventListener('message',event=>{if(event.source!==parent)return;const data=event.data;
if(data?.kind==='vorkspace.reply'){const item=pending.get(data.id);if(!item)return;pending.delete(data.id);if(data.error)item.reject(new Error(data.error));else item.resolve(data.result)}
if(data?.kind==='vorkspace.visibility'){dispatchEvent(new CustomEvent('vorkspace-visibility',{detail:data.visible}))}
if(data?.kind==='vorkspace.theme'){const style=document.documentElement.style;style.setProperty('--foreground',data.foreground);style.setProperty('--accent',data.accent);style.setProperty('--link',data.accent);style.setProperty('--background',data.light?'#fafafa':'#141414');style.setProperty('--surface',data.light?'#eee':'#222');style.setProperty('--muted',data.light?'#737373':'#969696');style.setProperty('--border',data.light?'#d4d4d4':'#383838');style.colorScheme=data.light?'light':'dark'}
});})();
</script>"#;
    let mut html = sandbox(&format!("{sdk}{body}"));
    let nonce = serde_json::to_string(nonce).unwrap();
    html.push_str(&format!(r#"<script>
addEventListener('message',event=>{{const frame=document.querySelector('iframe');if(event.source!==frame.contentWindow||event.origin!=='null'||event.data?.kind!=='vorkspace.request')return;
const data=event.data;if(!Number.isSafeInteger(data.id)||!['host','plugin'].includes(data.route)||typeof data.method!=='string')return;
const encoded=JSON.stringify({{nonce:{nonce},id:data.id,route:data.route,method:data.method,params:data.params}});if(encoded.length>{limit})return;
window.webkit.messageHandlers.vorkspacePlugin.postMessage(encoded);
}});
</script>"#,limit=vorkspace_plugin::MAX_FRAME));
    html
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
    pub(crate) fn new(body: String) -> Self {
        Self {
            bridge: None,
            body,
            interactive: true,
            markup: None,
            view: Rc::new(RefCell::new(None)),
        }
    }
    pub(crate) fn plugin(body: String, handler: RequestHandler) -> Self {
        let mut html = Self::new(body);
        html.bridge = Some(Rc::new(Bridge {
            nonce: uuid::Uuid::new_v4().simple().to_string(),
            handler,
        }));
        html
    }
    pub(crate) fn send_event(&self, value: serde_json::Value) {
        if let Some(view) = self.view.borrow().as_ref() {
            let script = format!(
                "document.querySelector('iframe').contentWindow.postMessage({},'*')",
                value
            );
            unsafe {
                view.web
                    .evaluateJavaScript_completionHandler(&NSString::from_str(&script), None);
            }
        }
    }
    fn markup(&mut self) -> Arc<str> {
        let ink = vorkspace_ui::theme::content(1.);
        let accent = vorkspace_ui::theme::accent_color();
        if self
            .markup
            .as_ref()
            .is_none_or(|m| self.bridge.is_none() && (m.ink != ink || m.accent != accent))
        {
            self.markup = Some(Markup {
                ink,
                accent,
                html: self
                    .bridge
                    .as_ref()
                    .map_or_else(
                        || sandbox(&self.body),
                        |bridge| plugin_sandbox(&self.body, &bridge.nonce),
                    )
                    .into(),
            });
        }
        self.markup.as_ref().unwrap().html.clone()
    }
    pub(crate) fn set_interactive(&mut self, interactive: bool) {
        if self.interactive != interactive {
            self.interactive = interactive;
        }
    }
    pub(crate) fn hide(&self) {
        if self.bridge.is_some() {
            self.send_event(serde_json::json!({"kind":"vorkspace.visibility","visible":false}));
        }
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
            let bridge = self.bridge.clone();
            let theme = (
                vorkspace_ui::theme::content(1.),
                vorkspace_ui::theme::accent_color(),
            );
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
                        // Generated artifacts get no handlers. Explicit plugins
                        // receive only their validated request bridge, never app IPC.
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
                            let messages=bridge.as_ref().map(|bridge|{
                                let messages=MessageGuard::alloc(mtm).set_ivars(bridge.clone());
                                let messages:Retained<MessageGuard>=msg_send![super(messages),init];
                                config.userContentController().addScriptMessageHandler_name(ProtocolObject::from_ref(&*messages),&NSString::from_str("vorkspacePlugin"));
                                messages
                            });
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
                                _messages:messages,
                                theme,
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
                        // Paint order is stacking order: a departing page painted last
                        // has to rise back above the page that arrived after it.
                        let frontmost = parent
                            .subviews()
                            .lastObject()
                            .is_some_and(|last| std::ptr::eq(&*last, &**view.clip));
                        if !frontmost {
                            parent.addSubview_positioned_relativeTo(
                                &view.clip,
                                NSWindowOrderingMode::Above,
                                None,
                            );
                        }
                        if bridge.is_some() && view.theme!=theme {
                            let color=|v:gpui::Hsla|{let c:gpui::Rgba=v.into();format!("rgba({},{},{},{})",(c.r*255.)as u8,(c.g*255.)as u8,(c.b*255.)as u8,c.a)};
                            let data=serde_json::json!({"kind":"vorkspace.theme","foreground":color(theme.0),"accent":color(theme.1),"light":vorkspace_ui::theme::is_light()});
                            let script=format!("document.querySelector('iframe').contentWindow.postMessage({},'*')",data);
                            unsafe{view.web.evaluateJavaScript_completionHandler(&NSString::from_str(&script),None);}
                            view.theme=theme;
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
                            if bridge.is_some(){unsafe{view.web.evaluateJavaScript_completionHandler(&NSString::from_str("document.querySelector('iframe').contentWindow.postMessage({kind:'vorkspace.visibility',visible:true},'*')"),None);}}
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
    fn plugin_bridge_requires_an_explicit_surface_and_an_opaque_child() {
        let plain = sandbox("<p>Artifact</p>");
        let plugin = plugin_sandbox("<p>Plugin</p>", "private-fixture-nonce");
        assert!(!plain.contains("vorkspacePlugin"));
        assert!(!plain.contains("window.vorkspace="));
        assert!(plugin.contains("sandbox=\"allow-scripts\""));
        assert!(!plugin.contains("allow-same-origin"));
        assert!(plugin.contains("event.source!==frame.contentWindow"));
        assert!(plugin.contains("event.origin!=='null'"));
        assert!(plugin.contains("nonce:\"private-fixture-nonce\""));
        // Optional output feeds the real WebKit fixture without maintaining a
        // second implementation of the opaque wrapper or JavaScript SDK.
        if let Some(output) = std::env::var_os("VORKSPACE_PLUGIN_DOCUMENTS") {
            let output = std::path::PathBuf::from(output);
            assert!(output.is_absolute() && output.is_dir());
            let body = format!(
                "{}{}",
                include_str!("../../../vorkspace-meetings-plugin/src/ui.html"),
                r#"<script>
addEventListener('message', async event => {
 if(event.source!==parent || event.data?.kind!=='fixture.run')return;
 const command=event.data;let value;
 try {
 if(command.action==='first'){document.querySelector('#list button').click();value=true;}
 if(command.action==='click'){document.getElementById(command.element).click();value=true;}
 if(command.action==='field'){const input=document.getElementById(command.element);input.value=command.value;input.dispatchEvent(new Event('input'));input.dispatchEvent(new Event('change'));value=true;}
 if(command.action==='inspect'){value={metadata:document.getElementById('metadata').innerText,settingsStop:{text:document.getElementById('settings-stop').innerText,width:document.getElementById('settings-stop').getBoundingClientRect().width,disabled:document.getElementById('settings-stop').disabled},text:document.body.innerText,title:document.getElementById('title').value,editor:document.getElementById('editor').value,preview:document.getElementById('preview').innerHTML,record:document.getElementById('record').innerText,summaryDisabled:document.getElementById('summarize').disabled,overflow:document.documentElement.scrollWidth>innerWidth,finals:document.getElementById('finals').innerText,notice:document.getElementById('notice').innerText};}
 if(command.action==='isolation'){let isolated=false;try{void parent.document.body}catch{isolated=true};value={isolated};}
 await vorkspace.host('fixture.report',{id:command.id,value});
 }catch(error){await vorkspace.host('fixture.report',{id:command.id,error:String(error)});}
});</script>"#
            );
            for light in [false, true] {
                vorkspace_ui::theme::configure(
                    &vorkspace_ui::preferences::Preferences {
                        theme: if light { "light" } else { "dark" }.into(),
                        ..Default::default()
                    },
                    light,
                    true,
                );
                std::fs::write(
                    output.join(format!(
                        "meetings-{}.html",
                        if light { "light" } else { "dark" }
                    )),
                    plugin_sandbox(&body, "private-fixture-nonce"),
                )
                .unwrap();
            }
            std::fs::write(output.join("artifact.html"), plain).unwrap();
            vorkspace_ui::theme::configure(&Default::default(), false, true);
        }
    }
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
