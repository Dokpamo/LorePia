use super::*;

#[test]
fn reserves_actual_bytes_and_holds_them_through_response_conversion() {
    let admission = AssetProtocolAdmission::new(4, 128);
    let mut responses = Vec::new();
    for _ in 0..4 {
        let mut permit = admission.try_acquire().expect("small response work slot");
        assert!(permit.reserve_bytes(8));
        let mut response = Response::new(vec![0; 8]);
        retain_permit(&mut response, permit);
        let (parts, body) = response.into_parts();
        responses.push(Response::from_parts(
            parts,
            std::borrow::Cow::<'static, [u8]>::Owned(body),
        ));
    }
    assert_eq!(admission.reserved_bytes(), 32);
    assert!(admission.try_acquire().is_none());
    responses.pop();
    assert_eq!(admission.reserved_bytes(), 24);
    assert!(admission.try_acquire().is_some());
    drop(responses);
    assert_eq!(admission.reserved_bytes(), 0);
}

#[test]
fn exhausted_body_budget_waits_until_a_response_is_consumed() {
    let admission = AssetProtocolAdmission::new(4, 128);
    let mut first = admission.try_acquire().expect("first work slot");
    assert!(first.reserve_bytes(128));
    let mut waiter = admission.try_acquire().expect("second work slot");
    let (sender, receiver) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        assert!(waiter.reserve_bytes(8));
        sender.send(waiter).expect("reserved response");
    });
    assert!(receiver.recv_timeout(Duration::from_millis(10)).is_err());
    assert_eq!(admission.reserved_bytes(), 128);
    drop(first);
    let reserved = receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("released body budget wakes waiter");
    worker.join().expect("waiter finished");
    assert_eq!(admission.reserved_bytes(), 8);
    drop(reserved);
    assert_eq!(admission.reserved_bytes(), 0);
}

#[tokio::test]
async fn queued_small_request_waits_instead_of_failing_when_work_slots_are_busy() {
    let admission = AssetProtocolAdmission::new(1, 128);
    let active = admission.try_acquire().expect("active response");
    let queued = admission.try_queue().expect("bounded waiting slot");
    let waiting = tokio::spawn(queued.acquire());
    tokio::task::yield_now().await;
    assert!(!waiting.is_finished());
    drop(active);
    let mut permit = waiting
        .await
        .expect("queued request")
        .expect("available work slot");
    assert!(permit.reserve_bytes(8));
    assert_eq!(admission.reserved_bytes(), 8);
}

#[tokio::test]
async fn queue_is_bounded_and_cancellation_releases_its_slot() {
    let admission = AssetProtocolAdmission::new(1, 128);
    let active = admission.try_acquire().expect("active response");
    let mut queued: Vec<_> = (0..MAX_QUEUED_REQUESTS)
        .map(|_| admission.try_queue().expect("queue capacity"))
        .collect();
    assert!(admission.try_queue().is_none());
    let waiting = queued.pop().expect("last queued request");
    assert!(
        tokio::time::timeout(Duration::from_millis(10), waiting.acquire())
            .await
            .is_err()
    );
    assert!(admission.try_queue().is_some());
    drop(queued);
    drop(active);
    assert_eq!(admission.inner.work.available_permits(), 1);
    assert_eq!(
        admission.inner.queue.available_permits(),
        MAX_QUEUED_REQUESTS
    );
}

#[test]
fn oversize_and_double_reservations_do_not_consume_budget() {
    let admission = AssetProtocolAdmission::new(1, 128);
    let mut permit = admission.try_acquire().expect("work slot");
    assert!(!permit.reserve_bytes(129));
    assert_eq!(admission.reserved_bytes(), 0);
    assert!(permit.reserve_bytes(8));
    assert!(!permit.reserve_bytes(1));
    assert_eq!(admission.reserved_bytes(), 8);
}
