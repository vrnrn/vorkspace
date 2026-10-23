//! Optional process naming applies before WebKit copies each configuration.
//! Origins, data stores, permissions and navigation stay with the view owner.
use objc2::{msg_send, sel};
use objc2_foundation::NSString;
use objc2_web_kit::WKWebViewConfiguration;

pub(crate) fn name_process(configuration: &WKWebViewConfiguration) {
    unsafe {
        let supported: bool =
            msg_send![configuration, respondsToSelector: sel!(_setProcessDisplayName:)];
        if supported {
            let _: () = msg_send![configuration, _setProcessDisplayName: &*NSString::from_str("vorkspace Web Content")];
        }
    }
}
