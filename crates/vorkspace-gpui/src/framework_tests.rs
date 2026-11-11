//! Contracts exercised through the embedded framework’s public entry points.
use gpui::{TextRun, WindowTextSystem, px};

#[gpui::test]
fn changing_line_clamp_keeps_complete_wrapping_geometry(cx: &mut gpui::TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| cx.new(|_| gpui::EmptyView))
            .unwrap()
    });
    let source = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu";
    let run = TextRun {
        len: source.len(),
        ..Default::default()
    };
    for clamps in [
        [None, Some(1), Some(2), None, Some(3)],
        [Some(2), None, Some(3), Some(1), None],
    ] {
        window
            .update(cx, |_, window, cx| {
                for clamp in clamps {
                    let actual = window
                        .text_system()
                        .shape_text(
                            source.into(),
                            px(14.),
                            std::slice::from_ref(&run),
                            Some(px(50.)),
                            clamp,
                        )
                        .unwrap();
                    let reference = WindowTextSystem::new(cx.text_system().clone())
                        .shape_text(
                            source.into(),
                            px(14.),
                            std::slice::from_ref(&run),
                            Some(px(50.)),
                            clamp,
                        )
                        .unwrap();
                    assert_eq!(actual.len(), 1);
                    assert_eq!(actual[0].text.as_ref(), source);
                    assert_eq!(actual[0].wrap_boundaries, reference[0].wrap_boundaries);
                    match clamp {
                        Some(limit) => assert_eq!(actual[0].wrap_boundaries.len(), limit - 1),
                        None => assert!(actual[0].wrap_boundaries.len() > 3),
                    }
                }
            })
            .unwrap();
        cx.refresh().unwrap();
    }
}

#[test]
fn nonblocking_queue_iteration_stays_ended_and_a_new_iterator_sees_later_work() {
    use gpui::{Priority, queue::PriorityQueueReceiver};
    let (sender, receiver) = PriorityQueueReceiver::new();
    let mut first = receiver.clone().try_iter();
    assert!(first.next().is_none());
    sender.send(Priority::High, 7).unwrap();
    assert!(first.next().is_none());
    assert!(first.next().is_none());

    let mut next = receiver.try_iter();
    assert_eq!(next.next().unwrap().unwrap(), 7);
    sender.send(Priority::Low, 8).unwrap();
    drop(sender);
    assert_eq!(next.next().unwrap().unwrap(), 8);
    assert!(next.next().unwrap().is_err());
    assert!(next.next().is_none());
    assert!(next.next().is_none());
}

#[test]
fn queue_lock_paths_share_weighted_fifo_policy_and_drain_after_sender_drop() {
    use gpui::{Priority, queue::PriorityQueueReceiver};
    let mut sequences = Vec::new();
    for pop in [
        PriorityQueueReceiver::<usize>::try_pop,
        PriorityQueueReceiver::<usize>::spin_try_pop,
    ] {
        let (sender, mut receiver) = PriorityQueueReceiver::new();
        assert!(pop(&mut receiver).unwrap().is_none());
        for index in 0..300 {
            let priority = [Priority::High, Priority::Medium, Priority::Low][index % 3];
            sender.send(priority, index).unwrap();
        }
        drop(sender);
        let mut sequence = Vec::new();
        while let Ok(Some(value)) = pop(&mut receiver) {
            sequence.push(value);
        }
        assert_eq!(sequence.len(), 300);
        assert!(pop(&mut receiver).is_err());
        for priority in 0..3 {
            let rows: Vec<_> = sequence
                .iter()
                .copied()
                .filter(|row| row % 3 == priority)
                .collect();
            assert_eq!(rows, (priority..300).step_by(3).collect::<Vec<_>>());
        }
        sequences.push(sequence);
    }
    assert_eq!(sequences[0], sequences[1]);
}
