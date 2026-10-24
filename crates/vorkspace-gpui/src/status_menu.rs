//! Native menu-bar anchor; clipboard presentation uses native AppKit controls.
#[derive(Clone, Copy)]
pub(crate) enum Action {
    Clipboard,
    QuickComposer,
    Settings,
    Quit,
}
#[cfg(all(target_os = "macos", not(test)))]
pub(crate) use native::Status;
#[cfg(all(target_os = "macos", not(test)))]
mod native {
    use super::Action;
    use objc2::runtime::AnyObject;
    use objc2::{
        AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
        rc::Retained, sel,
    };
    use objc2_app_kit::{NSImage, NSStatusBar, NSStatusItem};
    use objc2_foundation::{NSData, NSObject, NSObjectProtocol, NSSize, NSString};
    use std::rc::Rc;
    define_class!(
        #[unsafe(super(NSObject))]
        #[name="VorkspaceClipboardStatusTarget"]
        #[thread_kind=MainThreadOnly]
        #[ivars=Rc<dyn Fn(Action)>]
        struct Target;
        unsafe impl NSObjectProtocol for Target {}
        impl Target {
            #[unsafe(method(openClipboard:))]
            fn open(&self, _: Option<&AnyObject>) { (self.ivars())(Action::Clipboard); }
        }
    );
    pub(crate) struct Status {
        item: Retained<NSStatusItem>,
        _target: Retained<Target>,
    }
    impl Status {
        pub(crate) fn new(run: Rc<dyn Fn(Action)>) -> Option<Self> {
            let mtm = MainThreadMarker::new()?;
            let item = NSStatusBar::systemStatusBar().statusItemWithLength(-1.);
            let target: Retained<Target> =
                unsafe { msg_send![super(Target::alloc(mtm).set_ivars(run)), init] };
            if let Some(button) = item.button(mtm) {
                let data = NSData::with_bytes(include_bytes!("../assets/menu-bar-icon.png"));
                if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
                    image.setSize(NSSize::new(18., 18.));
                    image.setTemplate(false);
                    button.setImage(Some(&image));
                } else {
                    button.setTitle(&NSString::from_str("v_"));
                }
                button.setToolTip(Some(&NSString::from_str("vorkspace Clipboard · ⌘⇧C")));
                unsafe {
                    button.setTarget(Some(&target));
                    button.setAction(Some(sel!(openClipboard:)));
                }
            }
            Some(Self {
                item,
                _target: target,
            })
        }
        pub(crate) fn anchor(&self) -> Option<(f64, f64)> {
            let mtm = MainThreadMarker::new()?;
            let button = self.item.button(mtm)?;
            let window = button.window()?;
            let rect = window.convertRectToScreen(button.convertRect_toView(button.bounds(), None));
            Some((rect.origin.x, rect.origin.y))
        }
    }
    impl Drop for Status {
        fn drop(&mut self) {
            NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
        }
    }
}
