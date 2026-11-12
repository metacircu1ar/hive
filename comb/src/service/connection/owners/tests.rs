use super::*;

#[tokio::test]
async fn an_already_completed_query_owner_cannot_become_successful_cleanup() {
    let (_dir, server, _) = crate::service::tests::setup(None);
    let state = server.state.clone();
    let task = tokio::spawn(async {});
    while !task.is_finished() {
        tokio::task::yield_now().await;
    }
    let mut owners = Owners {
        backfill: None,
        query: Some(task),
        stopping: false,
    };
    let error = owners.stop(&state).await.unwrap_err();
    assert_eq!(error.code, crate::ErrorCode::WorkerFailed);
    assert_eq!(
        state.health.check().unwrap_err().code,
        crate::ErrorCode::RecoveryRequired
    );
    assert!(owners.query.is_none());
    assert!(owners.stop(&state).await.is_ok());
}

#[tokio::test]
async fn explicit_query_owner_cancellation_stays_a_successful_shutdown() {
    let (_dir, server, _) = crate::service::tests::setup(None);
    let state = server.state.clone();
    let mut owners = Owners {
        backfill: None,
        query: Some(tokio::spawn(std::future::pending())),
        stopping: false,
    };
    owners.stop(&state).await.unwrap();
    assert!(state.health.check().is_ok());
}

#[tokio::test]
async fn a_failed_backfill_result_fences_health_before_serving_can_continue() {
    let (_dir, server, _) = crate::service::tests::setup(None);
    let state = server.state.clone();
    let mut owners = Owners {
        backfill: Some(tokio::spawn(async { Err(Error::worker_failed()) })),
        query: Some(tokio::spawn(std::future::pending())),
        stopping: false,
    };
    assert_eq!(
        owners.next(&state).await.unwrap_err().code,
        crate::ErrorCode::WorkerFailed
    );
    assert_eq!(
        state.health.check().unwrap_err().code,
        crate::ErrorCode::RecoveryRequired
    );
    assert!(owners.backfill.is_none());
    owners.stop(&state).await.unwrap();
}

#[tokio::test]
async fn a_query_owner_that_dies_while_serving_fences_health_and_is_observed_once() {
    let (_dir, server, _) = crate::service::tests::setup(None);
    let state = server.state.clone();
    let mut owners = Owners {
        backfill: None,
        query: Some(tokio::spawn(async {
            panic!("owner fixture");
        })),
        stopping: false,
    };
    let error = owners.next(&state).await.unwrap_err();
    assert_eq!(error.code, crate::ErrorCode::WorkerFailed);
    assert!(owners.query.is_none());
    assert_eq!(
        state.health.check().unwrap_err().code,
        crate::ErrorCode::RecoveryRequired
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(10), owners.next(&state))
            .await
            .is_err()
    );
}
