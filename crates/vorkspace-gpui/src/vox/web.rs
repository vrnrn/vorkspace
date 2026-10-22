//! This WebKit view only loads the per-call, capability-protected loopback page.
//! Generated documents use a completely separate opaque WebKit configuration.
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained,
    runtime::ProtocolObject,
};
use objc2_app_kit::NSView;
use objc2_foundation::{
    NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL, NSURLRequest,
};
use objc2_web_kit::{
    WKFrameInfo, WKMediaCaptureState, WKMediaCaptureType, WKNavigationAction,
    WKNavigationActionPolicy, WKNavigationDelegate, WKPermissionDecision, WKSecurityOrigin,
    WKUIDelegate, WKWebView, WKWebViewConfiguration, WKWebsiteDataStore,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
struct Permission {
    origin: String,
    active: Arc<AtomicBool>,
}
define_class!(
    #[unsafe(super(NSObject))]
    #[name = "VorkspaceVoicePermission"]
    #[thread_kind=MainThreadOnly]
    #[ivars=Permission]
    struct Guard;
    unsafe impl NSObjectProtocol for Guard {}
    unsafe impl WKNavigationDelegate for Guard {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        unsafe fn navigate(
            &self,
            _: &WKWebView,
            action: &WKNavigationAction,
            reply: &block2::DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            let url = unsafe { action.request().URL() }
                .and_then(|u| u.absoluteString())
                .map(|s| s.to_string());
            reply.call((if url.as_deref() == Some(self.ivars().origin.as_str()) {
                WKNavigationActionPolicy::Allow
            } else {
                WKNavigationActionPolicy::Cancel
            },));
        }
    }
    unsafe impl WKUIDelegate for Guard {
        #[unsafe(method(webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:))]
        unsafe fn microphone(
            &self,
            _: &WKWebView,
            origin: &WKSecurityOrigin,
            _: &WKFrameInfo,
            kind: WKMediaCaptureType,
            reply: &block2::DynBlock<dyn Fn(WKPermissionDecision)>,
        ) {
            let expected = format!("http://127.0.0.1:{}/", unsafe { origin.port() });
            let allowed = self.ivars().active.load(Ordering::Relaxed)
                && kind == WKMediaCaptureType::Microphone
                && unsafe { origin.host() }.to_string() == "127.0.0.1"
                && self.ivars().origin.starts_with(&expected);
            reply.call((if allowed {
                WKPermissionDecision::Grant
            } else {
                WKPermissionDecision::Deny
            },));
        }
    }
);
pub(super) struct Web {
    web: Retained<WKWebView>,
    _guard: Retained<Guard>,
}
impl Web {
    pub(super) fn new(
        url: &str,
        active: Arc<AtomicBool>,
        window: &gpui::Window,
    ) -> Result<Self, String> {
        let mtm = MainThreadMarker::new().ok_or("Voice must open on the main thread")?;
        let handle = HasWindowHandle::window_handle(window).map_err(|e| e.to_string())?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return Err("Voice requires macOS".into());
        };
        let parent = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
        unsafe {
            let config = WKWebViewConfiguration::new(mtm);
            config.setWebsiteDataStore(&WKWebsiteDataStore::nonPersistentDataStore(mtm));
            let _: () = msg_send![&config,setMediaTypesRequiringUserActionForPlayback:0usize];
            let allocated = Guard::alloc(mtm).set_ivars(Permission {
                origin: url.into(),
                active,
            });
            let guard: Retained<Guard> = msg_send![super(allocated), init];
            let web = WKWebView::initWithFrame_configuration(
                WKWebView::alloc(mtm),
                NSRect::new(NSPoint::new(0., 0.), NSSize::new(1., 1.)),
                &config,
            );
            web.setNavigationDelegate(Some(ProtocolObject::from_ref(&*guard)));
            web.setUIDelegate(Some(ProtocolObject::from_ref(&*guard)));
            web.setAlphaValue(0.01);
            parent.addSubview(&web);
            let url = NSURL::URLWithString(&NSString::from_str(url)).ok_or("Invalid voice URL")?;
            web.loadRequest(&NSURLRequest::requestWithURL(&url));
            Ok(Self { web, _guard: guard })
        }
    }
}
impl Drop for Web {
    fn drop(&mut self) {
        unsafe {
            self.web
                .setMicrophoneCaptureState_completionHandler(WKMediaCaptureState::None, None);
            self.web.stopLoading();
        }
        self.web.removeFromSuperview();
    }
}
