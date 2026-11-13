//! A coalesced signal from a native directory watch.
use core_foundation::{
    array::{CFArray, CFArrayRef},
    base::TCFType,
    string::CFString,
};
use dispatch2::DispatchQueue;
use std::{
    ffi::c_void,
    path::PathBuf,
    ptr::{self, NonNull},
    slice,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
};

const NO_DEFER: u32 = 0x0000_0002;
const WATCH_ROOT: u32 = 0x0000_0004;
const MUST_SCAN: u32 = 0x0000_0001;
const ROOT_CHANGED: u32 = 0x0000_0020;
const UNMOUNTED: u32 = 0x0000_0080;
const RECONCILE: u8 = 1;
const REOPEN: u8 = 2;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Change {
    Files,
    Reconcile,
    Reopen,
}

struct Signal {
    wake: async_channel::Sender<()>,
    changes: AtomicU8,
}

#[repr(C)]
struct StreamContext {
    version: isize,
    info: *mut c_void,
    retain: unsafe extern "C" fn(*const c_void) -> *const c_void,
    release: unsafe extern "C" fn(*const c_void),
    copy_description: Option<unsafe extern "C" fn(*const c_void) -> *const c_void>,
}

type Callback =
    unsafe extern "C" fn(*const c_void, *mut c_void, usize, *mut c_void, *const u32, *const u64);

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn FSEventStreamCreate(
        allocator: *const c_void,
        callback: Callback,
        context: *mut StreamContext,
        paths: CFArrayRef,
        since: u64,
        latency: f64,
        flags: u32,
    ) -> *mut c_void;
    fn FSEventStreamSetDispatchQueue(stream: *mut c_void, queue: *const DispatchQueue);
    fn FSEventStreamStart(stream: *mut c_void) -> u8;
    fn FSEventStreamStop(stream: *mut c_void);
    fn FSEventStreamInvalidate(stream: *mut c_void);
    fn FSEventStreamRelease(stream: *mut c_void);
}

pub struct Watch {
    stream: NonNull<c_void>,
    events: async_channel::Receiver<()>,
    signal: Arc<Signal>,
}

// The owner serializes every stream control operation. Native callbacks
// access only their separately retained, thread-safe Signal.
unsafe impl Send for Watch {}

impl Watch {
    pub fn start(paths: &[PathBuf]) -> Option<Self> {
        if paths.is_empty() {
            return None;
        }
        let strings: Vec<_> = paths
            .iter()
            .map(|path| path.to_str().map(CFString::new))
            .collect::<Option<_>>()?;
        let paths = CFArray::from_CFTypes(&strings);
        let (wake, events) = async_channel::bounded(1);
        let signal = Arc::new(Signal {
            wake,
            changes: AtomicU8::new(0),
        });
        let mut context = StreamContext {
            version: 0,
            info: Arc::as_ptr(&signal).cast_mut().cast(),
            retain,
            release,
            copy_description: None,
        };
        let queue = DispatchQueue::new("com.vorkspace.checkout-events", None);
        // The framework retains the paths, queue and callback context.
        unsafe {
            let stream = NonNull::new(FSEventStreamCreate(
                ptr::null(),
                publish,
                &mut context,
                paths.as_concrete_TypeRef(),
                u64::MAX,
                0.2,
                NO_DEFER | WATCH_ROOT,
            ))?;
            FSEventStreamSetDispatchQueue(stream.as_ptr(), &*queue);
            if FSEventStreamStart(stream.as_ptr()) == 0 {
                FSEventStreamInvalidate(stream.as_ptr());
                FSEventStreamRelease(stream.as_ptr());
                return None;
            }
            Some(Self {
                stream,
                events,
                signal,
            })
        }
    }

    pub async fn changed(&self) -> Change {
        // The sender lives with the stream, so this channel remains open.
        self.events.recv().await.expect("active file event stream");
        self.pending()
    }

    pub fn pending(&self) -> Change {
        let _ = self.events.try_recv();
        let flags = self.signal.changes.swap(0, Ordering::AcqRel);
        if flags & REOPEN != 0 {
            Change::Reopen
        } else if flags & RECONCILE != 0 {
            Change::Reconcile
        } else {
            Change::Files
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        // The stream was successfully started and is still scheduled.
        unsafe {
            FSEventStreamStop(self.stream.as_ptr());
            FSEventStreamInvalidate(self.stream.as_ptr());
            FSEventStreamRelease(self.stream.as_ptr());
        }
    }
}

unsafe extern "C" fn retain(info: *const c_void) -> *const c_void {
    unsafe { Arc::increment_strong_count(info.cast::<Signal>()) };
    info
}

unsafe extern "C" fn release(info: *const c_void) {
    unsafe { Arc::decrement_strong_count(info.cast::<Signal>()) };
}

unsafe extern "C" fn publish(
    _: *const c_void,
    info: *mut c_void,
    count: usize,
    _: *mut c_void,
    flags: *const u32,
    _: *const u64,
) {
    if count == 0 {
        return;
    }
    // FSEvents owns a retain on info and provides one flag per event.
    let signal = unsafe { &*info.cast::<Signal>() };
    let flags = unsafe { slice::from_raw_parts(flags, count) };
    let mut changes = 0;
    for &flag in flags {
        if flag & (ROOT_CHANGED | UNMOUNTED) != 0 {
            changes |= REOPEN;
        }
        if flag & MUST_SCAN != 0 {
            changes |= RECONCILE;
        }
    }
    signal.changes.fetch_or(changes, Ordering::Release);
    let _ = signal.wake.try_send(());
}
