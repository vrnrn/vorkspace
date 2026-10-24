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
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};
    use std::thread;

    impl Budget {
        fn used(&self) -> usize {
            self.inner.0.lock().unwrap().used
        }
    }
    fn frame(budget: &Budget, data: &[u8]) -> Frame {
        let mut frame = Frame::new(budget.clone());
        frame.extend(data).unwrap();
        frame
    }

    #[test]
    fn exact_limit_and_eof_preserve_every_byte() {
        let budget = Budget::with_limits(8, 16);
        for input in [b"1234567\n".as_slice(), b"12345678".as_slice()] {
            let mut reader = Cursor::new(input);
            let output = read_line(&mut reader, &budget).unwrap().unwrap();
            assert_eq!(&*output, input);
            assert_eq!(budget.used(), 8);
            drop(output);
            assert_eq!(budget.used(), 0);
            assert!(read_line(&mut reader, &budget).unwrap().is_none());
        }
    }

    #[test]
    fn oversized_line_fails_before_consuming_or_allocating_the_remainder() {
        let budget = Budget::with_limits(8, 16);
        for input in [b"12345678\n".to_vec(), vec![b'x'; 1024 * 1024]] {
            let mut reader = BufReader::with_capacity(4, Cursor::new(input));
            let error = read_line(&mut reader, &budget).err().unwrap();
            assert!(error.contains("8 byte limit"));
            assert!(error.contains("not truncated"));
            assert!(
                reader.get_ref().position() <= 12,
                "overlong payload must not be drained"
            );
            assert_eq!(
                budget.used(),
                0,
                "failed frame must release its reservation"
            );
        }
    }

    #[test]
    fn dequeued_frames_keep_admission_until_consumer_finishes() {
        let budget = Budget::with_limits(16, 32);
        let (send, receive) = channel(budget.clone());
        send.send(frame(&budget, &[b'a'; 10])).unwrap();
        send.send(frame(&budget, &[b'b'; 10])).unwrap();
        let first = receive.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(budget.used(), 32);
        let producer_budget = budget.clone();
        let producer = thread::spawn(move || {
            let next = frame(&producer_budget, b"next");
            send.send(next).unwrap();
        });
        budget.await_waiter();
        assert_eq!(&*first, &[b'a'; 10]);
        assert_eq!(budget.used(), 32);
        drop(first);
        producer.join().unwrap();
        assert_eq!(budget.used(), 32);
        assert_eq!(
            &*receive.recv_timeout(Duration::from_secs(5)).unwrap(),
            &[b'b'; 10]
        );
        assert_eq!(
            &*receive.recv_timeout(Duration::from_secs(5)).unwrap(),
            b"next"
        );
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn growing_frame_admits_old_and_new_allocations_before_copying() {
        let budget = Budget::with_limits(32768, 65536);
        let mut growing = frame(&budget, &[b'a'; READ_BYTES]);
        let retained = frame(&budget, &[b'b'; 32768]);
        let producer = thread::spawn(move || {
            growing.extend(&[b'c'; 3 * READ_BYTES]).unwrap();
            growing
        });
        budget.await_waiter();
        assert_eq!(budget.used(), 40960);
        drop(retained);
        let growing = producer.join().unwrap();
        assert_eq!(budget.used(), 32768);
        assert_eq!(&growing[..READ_BYTES], &[b'a'; READ_BYTES]);
        assert_eq!(&growing[READ_BYTES..], &[b'c'; 3 * READ_BYTES]);
        drop(growing);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn dropping_receiver_wakes_byte_admission_and_drops_queued_payloads() {
        let budget = Budget::with_limits(8, 16);
        let (send, receive) = channel(budget.clone());
        send.send(frame(&budget, b"first")).unwrap();
        send.send(frame(&budget, b"second")).unwrap();
        let producer_budget = budget.clone();
        let producer = thread::spawn(move || {
            let mut pending = Frame::new(producer_budget);
            pending.extend(b"third").unwrap_err()
        });
        budget.await_waiter();
        drop(receive);
        assert!(producer.join().unwrap().contains("closed"));
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn receiver_close_releases_an_incomplete_frame_waiting_to_grow() {
        let budget = Budget::with_limits(32768, 65536);
        let (send, receive) = channel(budget.clone());
        send.send(frame(&budget, &[b'a'; 32768])).unwrap();
        let mut partial = frame(&budget, &[b'b'; READ_BYTES]);
        let producer = thread::spawn(move || {
            assert!(partial
                .extend(&[b'c'; 3 * READ_BYTES])
                .unwrap_err()
                .contains("closed"));
        });
        budget.await_waiter();
        assert_eq!(budget.used(), 40960);
        drop(receive);
        producer.join().unwrap();
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn closing_receiver_unblocks_count_admission_even_with_a_live_sender() {
        let budget = Budget::with_limits(8, 16);
        let (send, mut receive) = channel::<usize>(budget);
        for index in 0..QUEUED_FRAMES {
            send.send(index).unwrap();
        }
        assert!(matches!(
            send.try_send(99),
            Err(mpsc::TrySendError::Full(99))
        ));
        let producer = thread::spawn(move || send.send(100));
        receive.close();
        assert!(producer.join().unwrap().is_err());
        assert!(matches!(
            receive.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }
}
