use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Condvar, Mutex,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_waiter_does_not_release_a_live_blocking_jobs_permit() {
    let semaphore = Arc::new(Semaphore::new(1));
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let (started, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let task = {
        let release = release.clone();
        let semaphore = semaphore.clone();
        tokio::spawn(run_with(semaphore, move || {
            started.send(()).unwrap();
            let (lock, ready) = &*release;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = ready.wait(released).unwrap();
            }
            Ok(())
        }))
    };
    started_rx.recv().await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let available_while_active = semaphore.available_permits();
    let premature = semaphore.clone().try_acquire_owned();
    let prematurely_released = premature.is_ok();
    drop(premature);
    {
        let (lock, ready) = &*release;
        *lock.lock().unwrap() = true;
        ready.notify_all();
    }
    let permit = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        semaphore.clone().acquire_owned(),
    )
    .await
    .unwrap()
    .unwrap();
    drop(permit);
    assert_eq!(available_while_active, 0);
    assert!(!prematurely_released);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acquire_precedes_spawn_and_outputs_hold_the_budget_until_released() {
    let semaphore = Arc::new(Semaphore::new(1));
    let held = semaphore.clone().acquire_owned().await.unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let task = {
        let calls = calls.clone();
        tokio::spawn(run_with(semaphore.clone(), move || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(42)
        }))
    };
    tokio::task::yield_now().await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    drop(held);
    let (value, permit) = task.await.unwrap().unwrap();
    assert_eq!(value, 42);
    assert_eq!(semaphore.available_permits(), 0);
    drop(permit);
    assert_eq!(semaphore.available_permits(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_and_decompression_jobs_share_a_strict_concurrency_bound() {
    let semaphore = Arc::new(Semaphore::new(CONCURRENT_JOBS));
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let (started, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let (active, peak, release, started) = (
            active.clone(),
            peak.clone(),
            release.clone(),
            started.clone(),
        );
        let semaphore = semaphore.clone();
        tasks.push(tokio::spawn(async move {
            let ((), permit) = run_with(semaphore, move || {
                let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(count, Ordering::SeqCst);
                started.send(()).unwrap();
                let (lock, ready) = &*release;
                let mut released = lock.lock().unwrap();
                while !*released {
                    released = ready.wait(released).unwrap();
                }
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            })
            .await
            .unwrap();
            drop(permit);
        }));
    }
    for _ in 0..CONCURRENT_JOBS {
        receiver.recv().await.unwrap();
    }
    let active_before_release = active.load(Ordering::SeqCst);
    let available_before_release = semaphore.available_permits();
    {
        let (lock, ready) = &*release;
        *lock.lock().unwrap() = true;
        ready.notify_all();
    }
    for task in tasks {
        task.await.unwrap();
    }
    assert_eq!(active_before_release, CONCURRENT_JOBS);
    assert_eq!(available_before_release, 0);
    assert_eq!(peak.load(Ordering::SeqCst), CONCURRENT_JOBS);
    assert_eq!(active.load(Ordering::SeqCst), 0);
}
