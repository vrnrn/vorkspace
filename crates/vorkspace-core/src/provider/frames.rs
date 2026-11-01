//! Encoded provider frames share a byte budget while reading, queued and in use.
use std::io::BufRead;
use std::ops::Deref;
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::Duration;

// Full history and tool snapshots can be much larger than streaming deltas.
// Keep that protocol intact while bounding a faulty or runaway producer.
pub(super) const FRAME_BYTES: usize = 128 * 1024 * 1024;
pub(super) const WIRE_BYTES: usize = 256 * 1024 * 1024;
pub(super) const QUEUED_FRAMES: usize = 16;
const READ_BYTES: usize = 8192;

#[derive(Default)]
struct State {
    used: usize,
    waiting: usize,
    closed: bool,
}
#[derive(Clone)]
pub(super) struct Budget {
    inner: Arc<(Mutex<State>, Condvar)>,
    limit: usize,
    frame_limit: usize,
}
impl Budget {
    pub fn new() -> Self {
        Self::with_limits(FRAME_BYTES, WIRE_BYTES)
    }
    pub(super) fn with_limits(frame_limit: usize, limit: usize) -> Self {
        assert!(frame_limit > 0 && limit >= frame_limit.saturating_mul(2));
        Self {
            inner: Arc::new((Mutex::new(State::default()), Condvar::new())),
            limit,
            frame_limit,
        }
    }
    fn reserve(&self, bytes: usize) -> Result<(), String> {
        let (lock, ready) = &*self.inner;
        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
        while !state.closed && bytes > self.limit - state.used {
            state.waiting += 1;
            ready.notify_all();
            state = ready.wait(state).unwrap_or_else(|e| e.into_inner());
            state.waiting -= 1;
        }
        if state.closed {
            return Err("Provider output transport closed".into());
        }
        state.used += bytes;
        Ok(())
    }
    fn release(&self, bytes: usize) {
        let (lock, ready) = &*self.inner;
        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
        state.used -= bytes;
        ready.notify_all();
    }
    pub fn close(&self) {
        let (lock, ready) = &*self.inner;
        lock.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
        ready.notify_all();
    }
    #[cfg(test)]
    pub(super) fn await_waiter(&self) {
        let (lock, ready) = &*self.inner;
        let state = lock.lock().unwrap();
        let (state, timeout) = ready
            .wait_timeout_while(state, Duration::from_secs(5), |state| state.waiting == 0)
            .unwrap();
        assert!(
            !timeout.timed_out(),
            "producer never reached byte admission"
        );
        assert!(state.used <= self.limit);
    }
    fn oversized(&self) -> String {
        format!(
            "Provider output frame exceeds the {} byte limit ({} MiB). Reduce tool output or start a new conversation before retrying; the frame was not truncated.",
            self.frame_limit,
            self.frame_limit / (1024 * 1024)
        )
    }
}

pub(super) struct Frame {
    bytes: Vec<u8>,
    budget: Budget,
    reserved: usize,
}
impl std::fmt::Debug for Frame {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output
            .debug_struct("ProviderFrame")
            .field("bytes", &self.bytes.len())
            .finish()
    }
}
impl Frame {
    pub fn new(budget: Budget) -> Self {
        Self {
            bytes: Vec::new(),
            budget,
            reserved: 0,
        }
    }
    pub fn extend(&mut self, bytes: &[u8]) -> Result<(), String> {
        let required = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .filter(|required| *required <= self.budget.frame_limit)
            .ok_or_else(|| self.budget.oversized())?;
        if required > self.bytes.capacity() {
            let capacity = required
                .max(self.bytes.capacity().saturating_mul(2))
                .max(READ_BYTES.min(self.budget.frame_limit))
                .min(self.budget.frame_limit);
            // During growth the old and new allocations coexist. Admit the
            // entire replacement, not only its size difference, before copying.
            self.budget.reserve(capacity)?;
            let mut replacement = Vec::new();
            if replacement.try_reserve_exact(capacity).is_err() {
                self.budget.release(capacity);
                return Err("Cannot allocate the bounded provider output frame".into());
            }
            if replacement.capacity() > capacity {
                drop(replacement);
                self.budget.release(capacity);
                return Err("Provider frame allocation exceeded its byte reservation".into());
            }
            replacement.extend_from_slice(&self.bytes);
            drop(std::mem::replace(&mut self.bytes, replacement));
            self.budget.release(self.reserved);
            self.reserved = capacity;
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
}
impl Deref for Frame {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.bytes
    }
}
impl Drop for Frame {
    fn drop(&mut self) {
        // Drop the actual allocation before allowing another producer to use
        // the released bytes.
        drop(std::mem::take(&mut self.bytes));
        self.budget.release(self.reserved);
    }
}

/// Stop at the first oversized line, including an unterminated one. Only the
/// fixed-size BufReader read-ahead may exceed the accepted payload length.
pub(super) fn read_line(
    reader: &mut impl BufRead,
    budget: &Budget,
) -> Result<Option<Frame>, String> {
    let mut frame = Frame::new(budget.clone());
    Ok(extend_line(reader, &mut frame)?.then_some(frame))
}
fn extend_line(reader: &mut impl BufRead, frame: &mut Frame) -> Result<bool, String> {
    let start = frame.len();
    loop {
        let available = reader.fill_buf().map_err(|e| e.to_string())?;
        if available.is_empty() {
            return Ok(frame.len() != start);
        }
        let count = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |position| position + 1);
        let ended = available[count - 1] == b'\n';
        frame.extend(&available[..count])?;
        reader.consume(count);
        if ended {
            return Ok(true);
        }
    }
}

pub(super) struct Receiver<T> {
    receiver: Option<mpsc::Receiver<T>>,
    budget: Budget,
}
impl<T> Receiver<T> {
    pub fn recv_timeout(&self, timeout: Duration) -> Result<T, mpsc::RecvTimeoutError> {
        self.receiver
            .as_ref()
            .ok_or(mpsc::RecvTimeoutError::Disconnected)?
            .recv_timeout(timeout)
    }
    pub fn try_recv(&self) -> Result<T, mpsc::TryRecvError> {
        self.receiver
            .as_ref()
            .ok_or(mpsc::TryRecvError::Disconnected)?
            .try_recv()
    }
    pub fn close(&mut self) {
        self.budget.close();
        self.receiver.take();
    }
}
impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        self.close();
    }
}
pub(super) fn channel<T>(budget: Budget) -> (mpsc::SyncSender<T>, Receiver<T>) {
    let (sender, receiver) = mpsc::sync_channel(QUEUED_FRAMES);
    (
        sender,
        Receiver {
            receiver: Some(receiver),
            budget,
        },
    )
}

#[cfg(test)]
mod tests;
