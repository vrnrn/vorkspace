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
