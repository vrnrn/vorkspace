//! Encoded provider frames share a byte budget while reading, queued and in use.
use std::io::{BufRead, Read};
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

/// Bound a complete SSE record, not just its individual data lines. Compact
/// data fields in place so multiline events never allocate a second payload.
pub(super) fn read_sse(
    reader: &mut impl BufRead,
    budget: &Budget,
) -> Result<Option<Frame>, String> {
    let mut frame = Frame::new(budget.clone());
    loop {
        let start = frame.len();
        if !extend_line(reader, &mut frame)? {
            return if frame.is_empty() {
                Ok(None)
            } else {
                Err("OpenCode event stream ended within a frame".into())
            };
        }
        if matches!(&frame[start..], b"\n" | b"\r\n") {
            let mut input = 0;
            let mut output = 0;
            while input < frame.bytes.len() {
                let next = frame.bytes[input..]
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .map_or(frame.bytes.len(), |position| input + position + 1);
                let mut end = next;
                while end > input && matches!(frame.bytes[end - 1], b'\r' | b'\n') {
                    end -= 1;
                }
                if frame.bytes[input..end].starts_with(b"data:") {
                    let mut start = input + 5;
                    if frame.bytes.get(start) == Some(&b' ') {
                        start += 1;
                    }
                    let count = end - start;
                    frame.bytes.copy_within(start..end, output);
                    output += count;
                    frame.bytes[output] = b'\n';
                    output += 1;
                }
                input = next;
            }
            frame.bytes.truncate(output);
            return Ok(Some(frame));
        }
    }
}

/// Server logs are diagnostic text, not protocol frames. Retain only a bounded
/// prefix for readiness detection and drain the rest through the fixed buffer.
pub(super) fn read_log_line(reader: &mut impl BufRead) -> std::io::Result<Option<Vec<u8>>> {
    const RETAIN: usize = 65536;
    let mut line = Vec::new();
    let mut consumed = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(consumed.then_some(line));
        }
        consumed = true;
        let count = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |position| position + 1);
        let ended = available[count - 1] == b'\n';
        line.extend_from_slice(&available[..count.min(RETAIN - line.len())]);
        reader.consume(count);
        if ended {
            return Ok(Some(line));
        }
    }
}

pub(super) fn read_control_line(
    reader: &mut impl BufRead,
    limit: usize,
) -> std::io::Result<String> {
    let mut bytes = Vec::new();
    let count = reader
        .take((limit + 1) as u64)
        .read_until(b'\n', &mut bytes)?;
    if count > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Provider HTTP framing line exceeds its byte limit",
        ));
    }
    String::from_utf8(bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
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
    fn server_logs_drain_oversized_lines_with_a_bounded_retained_prefix() {
        let mut input = vec![b'x'; 1024 * 1024];
        input.extend_from_slice(b"\nlistening http://localhost:1234\n");
        let mut reader = BufReader::with_capacity(8192, Cursor::new(input));
        let prefix = read_log_line(&mut reader).unwrap().unwrap();
        assert_eq!(prefix.len(), 65536);
        assert!(prefix.capacity() <= 65536);
        assert_eq!(
            &read_log_line(&mut reader).unwrap().unwrap(),
            b"listening http://localhost:1234\n"
        );
        assert!(read_log_line(&mut reader).unwrap().is_none());
    }

    #[test]
    fn http_control_lines_fail_before_reading_an_unbounded_header() {
        let mut reader = BufReader::with_capacity(4, Cursor::new(vec![b'x'; 1024 * 1024]));
        assert!(read_control_line(&mut reader, 8)
            .unwrap_err()
            .to_string()
            .contains("byte limit"));
        assert!(reader.get_ref().position() <= 12);
        let mut reader = Cursor::new(b"3\r\n".as_slice());
        assert_eq!(read_control_line(&mut reader, 3).unwrap(), "3\r\n");
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

    #[test]
    fn multiline_sse_uses_one_budgeted_buffer_and_preserves_json() {
        let source = b": heartbeat\r\nevent: message\r\ndata: {\"text\":\r\ndata: \"hello\"}\r\n\r\ndata: 42\n\n";
        let budget = Budget::with_limits(128, 256);
        let mut reader = BufReader::with_capacity(3, Cursor::new(source));
        let event = read_sse(&mut reader, &budget).unwrap().unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&event).unwrap(),
            serde_json::json!({"text":"hello"})
        );
        assert_eq!(budget.used(), 128);
        drop(event);
        let event = read_sse(&mut reader, &budget).unwrap().unwrap();
        assert_eq!(&*event, b"42\n");
        drop(event);
        assert!(read_sse(&mut reader, &budget).unwrap().is_none());
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn sse_record_limit_applies_across_short_lines_and_comments() {
        let budget = Budget::with_limits(32, 64);
        for input in [
            "data: 1\ndata: 2\ndata: 3\ndata: 4\ndata: 5\n\n",
            ": long comment that keeps going without a delimiter",
        ] {
            let mut reader = BufReader::with_capacity(4, Cursor::new(input));
            assert!(read_sse(&mut reader, &budget)
                .err()
                .unwrap()
                .contains("32 byte limit"));
            assert!(reader.get_ref().position() <= 36);
            assert_eq!(budget.used(), 0);
        }
    }

    #[test]
    fn incomplete_sse_record_fails_and_empty_records_release_admission() {
        let budget = Budget::with_limits(32, 64);
        let mut reader = Cursor::new(b": heartbeat\n\n\ndata: {\"lost\":1}\n".as_slice());
        for _ in 0..2 {
            let empty = read_sse(&mut reader, &budget).unwrap().unwrap();
            assert!(empty.is_empty());
            drop(empty);
            assert_eq!(budget.used(), 0);
        }
        assert!(read_sse(&mut reader, &budget)
            .err()
            .unwrap()
            .contains("within a frame"));
        assert_eq!(budget.used(), 0);
    }
}
