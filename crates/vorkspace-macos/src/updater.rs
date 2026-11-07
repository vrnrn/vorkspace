//! Sparkle owns verification, staged installation and relaunch. Development
//! bundles have no feed or framework and never check the production channel.
mod macos {
    use objc2::{
        msg_send,
        rc::Retained,
        runtime::{AnyClass, AnyObject},
    };
    use objc2_foundation::{NSBundle, NSString};

    pub(super) struct Updater {
        _framework: Retained<NSBundle>,
        controller: Retained<AnyObject>,
    }
    impl gpui::Global for Updater {}
    pub(super) fn start(cx: &mut gpui::App) {
        let bundle = NSBundle::mainBundle();
        if bundle
            .objectForInfoDictionaryKey(&NSString::from_str("SUFeedURL"))
            .is_none()
        {
            return;
        }
        let path = format!(
            "{}/Contents/Frameworks/Sparkle.framework",
            bundle.bundlePath()
        );
        let Some(framework) = NSBundle::bundleWithPath(&NSString::from_str(&path)) else {
            return;
        };
        unsafe {
            if framework.loadAndReturnError().is_err() {
                return;
            }
            let Some(class) = AnyClass::get(c"SPUStandardUpdaterController") else {
                return;
            };
            let allocated: objc2::rc::Allocated<AnyObject> = msg_send![class, alloc];
            let controller: Option<Retained<AnyObject>> = msg_send![allocated, initWithStartingUpdater: true, updaterDelegate: std::ptr::null::<AnyObject>(), userDriverDelegate: std::ptr::null::<AnyObject>()];
            if let Some(controller) = controller {
                let updater: Retained<AnyObject> = msg_send![&controller, updater];
                let automatic: bool = msg_send![&updater, automaticallyChecksForUpdates];
                let can_check: bool = msg_send![&updater, canCheckForUpdates];
                // Check before Sparkle's first scheduled cycle, respecting an
                // explicit opt-out and leaving subsequent checks to Sparkle.
                if automatic && can_check {
                    let _: () = msg_send![&updater, checkForUpdatesInBackground];
                }
                cx.set_global(Updater {
                    _framework: framework,
                    controller,
                });
            }
        }
    }
    pub(super) fn check(cx: &gpui::App) -> bool {
        let Some(updater) = cx.try_global::<Updater>() else {
            return false;
        };
        unsafe {
            let _: () =
                msg_send![&updater.controller, checkForUpdates: std::ptr::null::<AnyObject>()];
        }
        true
    }
}
pub fn start(_cx: &mut gpui::App) {
    macos::start(_cx);
}
pub fn check(cx: &mut gpui::App) {
    if macos::check(cx) {
        return;
    }
    cx.open_url("https://github.com/vrnrn/vorkspace/releases");
}
