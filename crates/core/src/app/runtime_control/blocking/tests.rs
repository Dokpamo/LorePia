use super::*;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};
use tokio::sync::oneshot;

#[tokio::test(flavor = "current_thread")]
async fn slow_native_work_leaves_async_progress_and_cancellation_does_not_recycle_slots() {
    let work = BlockingWork::default();
    let (release_one, hold_one) = mpsc::channel();
    let (release_two, hold_two) = mpsc::channel();
    let (started_one, ready_one) = oneshot::channel();
    let (started_two, ready_two) = oneshot::channel();
    let running = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();
    for (hold, started) in [(hold_one, started_one), (hold_two, started_two)] {
        let work = work.clone();
        let running = Arc::clone(&running);
        let peak = Arc::clone(&peak);
        tasks.push(tokio::spawn(async move {
            work.run(move || {
                let count = running.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(count, Ordering::SeqCst);
                started.send(()).unwrap();
                // Bounds even a failing test; real DB work is not cancellable.
                let _ = hold.recv_timeout(Duration::from_secs(3));
                running.fetch_sub(1, Ordering::SeqCst);
            })
            .await
        }));
    }
    tokio::time::timeout(Duration::from_secs(1), async {
        ready_one.await.unwrap();
        ready_two.await.unwrap();
    })
    .await
    .unwrap();
    assert_eq!(running.load(Ordering::SeqCst), 2);
    // The single async runtime thread remains available with both native jobs held.
    tokio::task::yield_now().await;
    tasks.remove(0).abort();
    let (started_three, mut ready_three) = oneshot::channel();
    let next = tokio::spawn(async move { work.run(move || started_three.send(())).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut ready_three)
            .await
            .is_err()
    );
    release_one.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), &mut ready_three)
        .await
        .unwrap()
        .unwrap();
    release_two.send(()).unwrap();
    next.await.unwrap().unwrap().unwrap();
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    assert_eq!(peak.load(Ordering::SeqCst), 2);
}
