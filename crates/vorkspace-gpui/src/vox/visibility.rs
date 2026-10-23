//! Explicit native hide/minimize events end a call. Losing keyboard focus or
//! becoming occluded does not transfer ownership of the microphone.
use async_channel::Receiver;
use block2::RcBlock;
use objc2::{
    MainThreadMarker,
    rc::Retained,
    runtime::{AnyObject, NSObjectProtocol, ProtocolObject},
};
use objc2_app_kit::{NSApplication, NSView};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSString};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    ptr::NonNull,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const MINIMIZED: &str = "NSWindowDidMiniaturizeNotification";
const HIDDEN: &str = "NSApplicationDidHideNotification";

pub(super) struct Watch {
    center: Retained<NSNotificationCenter>,
    observers: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
    signal: async_channel::Sender<()>,
}

impl Watch {
    pub(super) fn new(
        window: &gpui::Window,
        active: Arc<AtomicBool>,
    ) -> Result<(Self, Receiver<()>), String> {
        let mtm =
            MainThreadMarker::new().ok_or("Vox must observe its window on the main thread")?;
        let handle = HasWindowHandle::window_handle(window).map_err(|error| error.to_string())?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return Err("Vox requires a native macOS window".into());
        };
        let view = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
        let window = view.window().ok_or("Vox window is unavailable")?;
        let app = NSApplication::sharedApplication(mtm);
        Ok(Self::observe(
            NSNotificationCenter::defaultCenter(),
            &window,
            &app,
            active,
        ))
    }

    fn observe(
        center: Retained<NSNotificationCenter>,
        window: &AnyObject,
        app: &AnyObject,
        active: Arc<AtomicBool>,
    ) -> (Self, Receiver<()>) {
        let (sender, receiver) = async_channel::bounded(1);
        let signal = sender.clone();
        let block = RcBlock::new(move |_: NonNull<NSNotification>| {
            // AppKit can notify while GPUI is already borrowed. Deny new voice
            // effects immediately, then let the call owner perform shutdown.
            active.store(false, Ordering::Relaxed);
            let _ = sender.try_send(());
        });
        let observers = [(MINIMIZED, window), (HIDDEN, app)]
            .into_iter()
            .map(|(name, object)| unsafe {
                center.addObserverForName_object_queue_usingBlock(
                    Some(&NSString::from_str(name)),
                    Some(object),
                    None,
                    &block,
                )
            })
            .collect();
        (
            Self {
                center,
                observers,
                signal,
            },
            receiver,
        )
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.signal.close();
        for observer in &self.observers {
            unsafe {
                self.center.removeObserver(observer.as_ref());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2::runtime::NSObject;

    fn post(center: &NSNotificationCenter, name: &str, object: &AnyObject) {
        unsafe {
            center.postNotificationName_object(&NSString::from_str(name), Some(object));
        }
    }

    #[test]
    fn only_explicit_hide_and_the_owning_windows_minimize_revoke_the_call() {
        for (event, use_app) in [(MINIMIZED, false), (HIDDEN, true)] {
            let center = NSNotificationCenter::new();
            let window = NSObject::new();
            let other = NSObject::new();
            let app = NSObject::new();
            let active = Arc::new(AtomicBool::new(true));
            let (_watch, signal) = Watch::observe(center.clone(), &window, &app, active.clone());
            for (name, object) in [
                ("NSWindowDidResignKeyNotification", &*window as &AnyObject),
                ("NSWindowDidChangeOcclusionStateNotification", &*window),
                (MINIMIZED, &*other),
            ] {
                post(&center, name, object);
            }
            assert!(active.load(Ordering::Relaxed));
            assert!(signal.try_recv().is_err());
            post(&center, event, if use_app { &app } else { &window });
            assert!(!active.load(Ordering::Relaxed));
            assert_eq!(signal.try_recv(), Ok(()));
        }
    }

    #[test]
    fn dropping_the_call_observer_unregisters_both_notifications_and_closes_the_signal() {
        let center = NSNotificationCenter::new();
        let window = NSObject::new();
        let app = NSObject::new();
        let active = Arc::new(AtomicBool::new(true));
        let (watch, signal) = Watch::observe(center.clone(), &window, &app, active.clone());
        drop(watch);
        post(&center, MINIMIZED, &window);
        post(&center, HIDDEN, &app);
        assert!(active.load(Ordering::Relaxed));
        assert!(signal.is_closed());
    }
}
