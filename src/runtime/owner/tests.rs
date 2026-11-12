use super::*;

fn boundary() -> (Boundary, Arc<Wake>, Arc<AtomicBool>, Arc<AtomicBool>) {
    let wake = Arc::new(Wake::default());
    let shutdown = Arc::new(AtomicBool::new(false));
    let uncertain = Arc::new(AtomicBool::new(false));
    (
        Boundary::new(wake.clone(), shutdown.clone(), uncertain.clone()),
        wake,
        shutdown,
        uncertain,
    )
}

#[tokio::test]
async fn abort_before_first_poll_publishes_failure_and_wakes_dependents() {
    let (boundary, wake, shutdown, uncertain) = boundary();
    let notification = wake.register("waiting");
    let task = tokio::spawn(async move {
        std::future::pending::<()>().await;
        boundary.finish(&Ok(()));
    });
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(shutdown.load(Ordering::Acquire));
    assert!(uncertain.load(Ordering::Acquire));
    assert_eq!(wake.failure().unwrap().code(), "daemon_stopping");
    for pending in [&wake.supervisor, &wake.deadlines, &notification] {
        tokio::time::timeout(std::time::Duration::from_millis(100), pending.notified())
            .await
            .unwrap();
    }
}

#[test]
fn ordinary_results_disarm_without_losing_the_primary_cause() {
    for failed in [false, true] {
        let (boundary, wake, shutdown, uncertain) = boundary();
        let result = if failed {
            Err(Error::database_error("primary"))
        } else {
            Ok(())
        };
        boundary.finish(&result);
        assert_eq!(shutdown.load(Ordering::Acquire), failed);
        assert!(!uncertain.load(Ordering::Acquire));
        assert_eq!(
            wake.failure().map(|error| error.code()),
            failed.then_some("database_error")
        );
    }
}
