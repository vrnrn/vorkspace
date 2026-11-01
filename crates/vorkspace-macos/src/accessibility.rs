//! System accessibility settings and spoken announcements.
use objc2::{MainThreadMarker, msg_send, rc::Retained, runtime::AnyObject};
use objc2_app_kit::{
    NSAccessibilityAnnouncementKey, NSAccessibilityAnnouncementRequestedNotification,
    NSAccessibilityPostNotificationWithUserInfo, NSAccessibilityPriorityKey, NSApplication,
    NSWorkspace,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString};

/// NSAccessibilityPriorityHigh: speech interrupts what VoiceOver is reading.
const PRIORITY_HIGH: isize = 90;

/// System Settings > Accessibility > Display > Reduce motion.
pub fn reduce_motion() -> bool {
    let workspace = NSWorkspace::sharedWorkspace();
    unsafe { msg_send![&workspace, accessibilityDisplayShouldReduceMotion] }
}

/// Speaks `text` through VoiceOver. It does nothing when VoiceOver is off.
pub fn announce(text: &str) {
    let Some(main_thread) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(main_thread);
    let message = NSString::from_str(text);
    let priority = NSNumber::new_isize(PRIORITY_HIGH);
    unsafe {
        let info: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(
            &[NSAccessibilityAnnouncementKey, NSAccessibilityPriorityKey],
            &[message.as_ref(), priority.as_ref()],
        );
        NSAccessibilityPostNotificationWithUserInfo(
            &app,
            NSAccessibilityAnnouncementRequestedNotification,
            Some(info.cast_unchecked()),
        );
    }
}
